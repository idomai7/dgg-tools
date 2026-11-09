use {
  ::logger::{
    capture::now_ms,
    control::{self, Status, parse_push, push_body},
    serve::router,
  },
  ::std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
  },
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("logger-serve-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn status(instance: &str) -> Status {
  Status {
    instance: instance.into(),
    started: 1_000,
    heartbeat: 2_000,
    healthy_since: Some(1_500),
    build: Some("0.1.0+abc".into()),
  }
}

#[test]
fn pushed_statuses_move_onto_the_receivers_clock() {
  let body = push_body(&status("a").encode(), 2_000);
  assert!(body.starts_with("now 2000\ncapture-status 0.0\n"));
  // The receiver's clock is 10 s ahead.
  let mut expected = status("a");
  expected.shift(10_000);
  assert_eq!(expected.heartbeat, 12_000);
  assert_eq!(parse_push(&body, 12_000), Some(expected));
  assert_eq!(parse_push("now x\n", 0), None);
  assert_eq!(parse_push(&status("a").encode(), 0), None);
}

#[::tokio::test]
async fn control_endpoints() {
  let root = TempRoot::new();
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let app = router(root.0.clone(), "secret".into());
  let server =
    ::tokio::spawn(async move { ::axum::serve(listener, app).await.unwrap() });
  let client = ::reqwest::Client::new();
  let url = |path: &str| format!("{base}{path}");
  let body = push_body(&status("remote").encode(), now_ms());

  let put = |path: &str, token: &str, body: String| {
    client.put(url(path)).bearer_auth(token).body(body).send()
  };
  let code = |response: ::reqwest::Response| response.status().as_u16();
  assert_eq!(
    code(put("/control/remote", "wrong", body.clone()).await.unwrap()),
    401
  );
  let unauthenticated = client.get(url("/control")).send().await.unwrap();
  assert_eq!(code(unauthenticated), 401);
  assert_eq!(
    code(put("/control/other", "secret", body.clone()).await.unwrap()),
    400
  );
  assert_eq!(
    code(
      put("/control/remote", "secret", "junk".into())
        .await
        .unwrap()
    ),
    400
  );
  assert_eq!(
    code(put("/control/remote", "secret", body).await.unwrap()),
    204
  );

  // It's written where local instances look.
  let control = control::dir(&root.0);
  let written = control::read_all(&control).unwrap();
  assert_eq!(written.len(), 1);
  assert_eq!(written[0].instance, "remote");
  let listed = client
    .get(url("/control"))
    .bearer_auth("secret")
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
  assert!(listed.starts_with("now "));
  assert!(listed.contains("\n\ncapture-status 0.0\ninstance remote\n"));

  let deleted = client
    .delete(url("/control/remote"))
    .bearer_auth("secret")
    .send()
    .await
    .unwrap();
  assert_eq!(code(deleted), 204);
  assert_eq!(control::read_all(&control).unwrap(), []);
  server.abort();
}
