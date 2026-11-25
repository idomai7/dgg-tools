use {
  ::common::temp_dir::TempDir,
  ::common::time::now_ms,
  ::logger::{
    cert,
    control::{self, Status, parse_push, push_body},
    dgg::Dgg,
    serve::{self, CONCURRENT, Tokens, router},
    service::Service,
  },
  ::logger_journal::{Writer, record::v0::Body},
  ::rcgen::KeyPair,
};

fn status(instance: &str) -> Status {
  Status {
    instance: instance.into(),
    started: 1_000,
    heartbeat: 2_000,
    healthy_since: Some(1_500),
    build: Some("0.1.0+abc".into()),
    listening: None,
  }
}

#[test]
fn pushed_statuses_move_onto_the_receivers_clock() {
  let body = push_body(&status("a").encode(), 2_000);
  assert!(body.starts_with("now 2000\ncapture-status dgg.0.2\u{1e}\n"));
  // The receiver's clock is 10 s ahead.
  let mut expected = status("a");
  expected.shift(10_000);
  assert_eq!(expected.heartbeat, 12_000);
  assert_eq!(parse_push(&body, 12_000), Some(expected));
  assert_eq!(parse_push("now x\n", 0), None);
  assert_eq!(parse_push(&status("a").encode(), 0), None);
}

#[test]
fn tokens_must_be_long_and_differ() {
  let long = |c: char| c.to_string().repeat(32);
  let tokens = |read: String, control: Option<String>| Tokens {
    read,
    control,
    query: None,
  };
  let query = |query: String| Tokens {
    query: Some(query),
    ..tokens(long('a'), Some(long('b')))
  };
  assert!(tokens(long('a'), None).check().is_ok());
  assert!(tokens(long('a'), Some(long('b'))).check().is_ok());
  assert!(query(long('c')).check().is_ok());
  for bad in [
    tokens("a".repeat(31), None),
    tokens(long('a'), Some("b".repeat(31))),
    tokens(long('a'), Some(long('a'))),
    query("c".repeat(31)),
    query(long('a')),
    query(long('b')),
  ] {
    assert!(bad.check().is_err(), "{bad:?}");
  }
}

/// Serves `root` with `tokens` on a free port; returns its URL.
async fn serve(root: &TempDir, tokens: Tokens) -> String {
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let app = router(root.to_path_buf(), tokens);
  ::tokio::spawn(async move { ::axum::serve(listener, app).await.unwrap() });
  base
}

#[::tokio::test]
async fn control_endpoints() {
  let root = TempDir::new("logger-serve");
  let tokens = Tokens {
    read: "reader".into(),
    control: Some("secret".into()),
    query: None,
  };
  let base = serve(&root, tokens).await;
  let client = ::reqwest::Client::new();
  let url = |path: &str| format!("{base}{path}");
  let body = push_body(&status("remote").encode(), now_ms());

  let put = |path: &str, token: &str, body: String| {
    client.put(url(path)).bearer_auth(token).body(body).send()
  };
  let code = |response: ::reqwest::Response| response.status().as_u16();
  assert_eq!(
    code(
      put(
        "/dgg/v0/channels/destiny.gg/control/remote",
        "wrong",
        body.clone()
      )
      .await
      .unwrap()
    ),
    401
  );
  let unauthenticated = client
    .get(url("/dgg/v0/channels/destiny.gg/control"))
    .send()
    .await
    .unwrap();
  assert_eq!(code(unauthenticated), 401);
  // The read token reads, but can't push.
  assert_eq!(
    code(
      put(
        "/dgg/v0/channels/destiny.gg/control/remote",
        "reader",
        body.clone()
      )
      .await
      .unwrap()
    ),
    403
  );
  let read = client
    .get(url("/dgg/v0/channels/destiny.gg/control"))
    .bearer_auth("reader")
    .send();
  assert_eq!(code(read.await.unwrap()), 200);
  assert_eq!(
    code(
      put(
        "/dgg/v0/channels/destiny.gg/control/other",
        "secret",
        body.clone()
      )
      .await
      .unwrap()
    ),
    400
  );
  assert_eq!(
    code(
      put(
        "/dgg/v0/channels/destiny.gg/control/remote",
        "secret",
        "junk".into()
      )
      .await
      .unwrap()
    ),
    400
  );
  assert_eq!(
    code(
      put("/dgg/v0/channels/destiny.gg/control/remote", "secret", body)
        .await
        .unwrap()
    ),
    204
  );

  // It's written where local instances look.
  let control = control::dir(&Dgg::dir(root.path()));
  let written = control::read_all(&control).unwrap();
  assert_eq!(written.len(), 1);
  assert_eq!(written[0].instance, "remote");
  let listed = client
    .get(url("/dgg/v0/channels/destiny.gg/control"))
    .bearer_auth("secret")
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
  assert!(listed.starts_with("now "));
  assert!(
    listed
      .contains("\n\ncapture-status dgg.0.2\u{1e}\ninstance remote\u{1e}\n")
  );

  let deleted = client
    .delete(url("/dgg/v0/channels/destiny.gg/control/remote"))
    .bearer_auth("secret")
    .send()
    .await
    .unwrap();
  assert_eq!(code(deleted), 204);
  assert_eq!(control::read_all(&control).unwrap(), []);
}

#[::tokio::test]
async fn without_a_control_token_nothing_pushes() {
  let root = TempDir::new("logger-serve");
  let tokens = Tokens {
    read: "reader".into(),
    control: None,
    query: None,
  };
  let base = serve(&root, tokens).await;
  let client = ::reqwest::Client::new();
  let body = push_body(&status("remote").encode(), now_ms());
  for token in ["reader", ""] {
    let pushed = client
      .put(format!("{base}/dgg/v0/channels/destiny.gg/control/remote"))
      .bearer_auth(token)
      .body(body.clone())
      .send()
      .await
      .unwrap();
    assert!([401, 403].contains(&pushed.status().as_u16()), "{token:?}");
  }
  let control = control::dir(&Dgg::dir(root.path()));
  assert_eq!(control::read_all(&control).unwrap(), []);
}

#[::tokio::test]
async fn serves_a_few_requests_at_once() {
  let root = TempDir::new("logger-serve");
  // Big enough that sending it stalls until it's read.
  let mut writer = Writer::new(Dgg::dir(root.path()), "a");
  let frame = "x".repeat(1 << 20);
  for _ in 0..64 {
    writer.write(0, 1, Body::Frame(frame.clone())).unwrap();
  }
  drop(writer);
  let tokens = Tokens {
    read: "reader".into(),
    control: None,
    query: None,
  };
  let base = serve(&root, tokens).await;
  let client = ::reqwest::Client::new();
  let get = || {
    client
      .get(format!(
        "{base}/dgg/v0/channels/destiny.gg/files/live/1970-01-01/a.journal"
      ))
      .bearer_auth("reader")
      .send()
  };
  let mut held = Vec::new();
  for _ in 0..CONCURRENT {
    let response = get().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    held.push(response);
  }
  assert_eq!(get().await.unwrap().status().as_u16(), 503);
  // One finished, there's room again.
  let size = held.pop().unwrap().bytes().await.unwrap().len();
  assert!(size > 64 << 20);
  assert_eq!(get().await.unwrap().status().as_u16(), 200);
}

/// Serves `root` over TLS on a free port, with a certificate for `host`
/// from a new CA; returns its URL and the CA's certificate.
async fn serve_tls(root: &TempDir, host: &str) -> (String, String) {
  let key = KeyPair::generate().unwrap();
  let ca_cert = cert::ca(&key).unwrap();
  let node = cert::node("node", &[host.into()], &ca_cert, &key).unwrap();
  let tls = cert::server_config(&node.cert, &node.key).unwrap();
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let base = format!("https://{}", listener.local_addr().unwrap());
  let tokens = Tokens {
    read: "reader".into(),
    control: None,
    query: None,
  };
  let app = router(root.to_path_buf(), tokens);
  ::tokio::spawn(
    async move { serve::run(listener, app, Some(tls)).await.unwrap() },
  );
  (base, ca_cert)
}

/// A client trusting only `ca_cert`.
fn trusting(ca_cert: &str) -> ::reqwest::Client {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let ca = ::reqwest::Certificate::from_pem(ca_cert.as_bytes()).unwrap();
  ::reqwest::Client::builder()
    .tls_certs_only([ca])
    .build()
    .unwrap()
}

#[::tokio::test]
async fn serves_over_tls() {
  let root = TempDir::new("logger-serve");
  let (base, ca_cert) = serve_tls(&root, "127.0.0.1").await;
  let manifest = |client: ::reqwest::Client, base: String| async move {
    client
      .get(format!("{base}/dgg/v0/channels/destiny.gg/manifest"))
      .bearer_auth("reader")
      .send()
      .await
  };
  let response = manifest(trusting(&ca_cert), base.clone()).await.unwrap();
  assert_eq!(response.status().as_u16(), 200);
  // Not to a client trusting another CA.
  let other = cert::ca(&KeyPair::generate().unwrap()).unwrap();
  assert!(manifest(trusting(&other), base.clone()).await.is_err());
  // Nor over plain HTTP.
  let plain = base.replace("https://", "http://");
  assert!(manifest(::reqwest::Client::new(), plain).await.is_err());
}

#[::tokio::test]
async fn a_certificate_for_another_host_is_refused() {
  let root = TempDir::new("logger-serve");
  let (base, ca_cert) = serve_tls(&root, "192.0.2.1").await;
  let sent = trusting(&ca_cert)
    .get(format!("{base}/dgg/v0/channels/destiny.gg/manifest"))
    .bearer_auth("reader")
    .send()
    .await;
  assert!(sent.is_err());
}
