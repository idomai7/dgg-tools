//! The page's server, on a port of its own, with a local server standing
//! in for Kick's. Slugs, ids and tokens are made up.

mod fake;

use {
  ::common::temp_dir::TempDir,
  ::kick_api::official::oauth::App,
  ::kick_mod::{account::Account, serve},
  ::reqwest::{
    Url,
    header::{HOST, LOCATION, ORIGIN},
    redirect::Policy,
  },
  ::std::fs,
  ::tokio::net::TcpListener,
  fake::Kick,
};

/// The server, with its files in a directory removed when the test ends.
struct Server {
  base: String,
  port: u16,
  dir: TempDir,
  kick: Kick,
  http: ::reqwest::Client,
}

impl Server {
  async fn start() -> Self {
    let dir = TempDir::new("kick-mod-serve");
    let kick = Kick::start().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut app = App::new("app-id", "app-secret");
    app.base = kick.base.clone();
    let redirect = format!("http://localhost:{port}/oauth/callback");
    let token = dir.path().join("token.json");
    let http = ::reqwest::Client::new();
    let account = Account::new(http, app, &kick.base, redirect, token);
    let channels = dir.path().join("channels.json");
    ::tokio::spawn(serve::run(listener, channels, account.unwrap()));
    Self {
      base: format!("http://127.0.0.1:{port}"),
      port,
      dir,
      kick,
      http: ::reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .unwrap(),
    }
  }

  async fn get(&self, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut request = self.http.get(format!("{}{path}", self.base));
    for &(name, value) in headers {
      request = request.header(name, value);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
  }

  /// Where `path` redirects to.
  async fn redirect(&self, path: &str) -> String {
    let response = self
      .http
      .get(format!("{}{path}", self.base))
      .send()
      .await
      .unwrap();
    assert_eq!(response.status().as_u16(), 303);
    response.headers()[LOCATION].to_str().unwrap().to_owned()
  }

  fn channels(&self) -> ::std::path::PathBuf {
    self.dir.path().join("channels.json")
  }
}

#[tokio::test]
async fn serves_the_page() {
  let server = Server::start().await;
  let (status, page) = server.get("/", &[]).await;
  assert_eq!(status, 200);
  assert!(page.contains("<title>kick-mod</title>"));
}

#[tokio::test]
async fn lists_the_channels_as_they_are_now() {
  let server = Server::start().await;
  assert_eq!(server.get("/api/channels", &[]).await, (200, "{}".into()));
  fs::write(
    server.channels(),
    r#"{"one": {"broadcaster_user_id": 1, "chatroom_id": null}}"#,
  )
  .unwrap();
  let (status, body) = server.get("/api/channels", &[]).await;
  assert_eq!(status, 200);
  assert_eq!(
    ::serde_json::from_str::<::serde_json::Value>(&body).unwrap(),
    ::serde_json::json!({"one": {"broadcaster_user_id": 1, "chatroom_id": null}})
  );
  fs::write(server.channels(), "{ not json").unwrap();
  assert_eq!(server.get("/api/channels", &[]).await.0, 500);
}

#[tokio::test]
async fn only_for_itself() {
  let server = Server::start().await;
  let port = server.port;
  let own = format!("127.0.0.1:{port}");
  let local = format!("localhost:{port}");
  let elsewhere = format!("example.com:{port}");
  for host in [&own, &local, &local.to_uppercase()] {
    assert_eq!(server.get("/", &[(HOST.as_str(), host)]).await.0, 200);
  }
  // A website's name pointed at this address: DNS rebinding.
  assert_eq!(server.get("/", &[(HOST.as_str(), &elsewhere)]).await.0, 403);
  assert_eq!(
    server.get("/", &[(HOST.as_str(), "127.0.0.1")]).await.0,
    403
  );

  for origin in [format!("http://{own}"), format!("http://{local}")] {
    let headers = [(ORIGIN.as_str(), &*origin)];
    assert_eq!(server.get("/api/channels", &headers).await.0, 200);
  }
  // A website's page asking it.
  for origin in [
    "https://example.com",
    &format!("http://{elsewhere}"),
    &format!("https://{own}"),
    "null",
  ] {
    let headers = [(ORIGIN.as_str(), origin)];
    let (status, body) = server.get("/api/channels", &headers).await;
    assert_eq!((status, &*body), (403, "not this server's"));
  }
}

#[tokio::test]
async fn signs_in_and_out() {
  let server = Server::start().await;
  let signed_out = r#"{"user":null}"#;
  assert_eq!(server.get("/api/me", &[]).await, (200, signed_out.into()));

  let to_kick = server.redirect("/sign-in").await;
  assert!(
    to_kick.starts_with(&format!("{}/oauth/authorize?", server.kick.base))
  );
  let asked: Vec<_> = Url::parse(&to_kick)
    .unwrap()
    .query_pairs()
    .into_owned()
    .collect();
  let get = |key: &str| asked.iter().find(|(k, _)| k == key).unwrap().1.clone();
  let port = server.port;
  assert_eq!(
    get("redirect_uri"),
    format!("http://localhost:{port}/oauth/callback")
  );

  server.kick.answer(
    "/oauth/token",
    200,
    r#"{"access_token":"access-1","refresh_token":"refresh-1","expires_in":3600}"#,
  );
  server.kick.answer(
    "/users",
    200,
    r#"{"data":[{"user_id":7,"name":"moderator"}],"message":"OK"}"#,
  );
  let back = format!("/oauth/callback?code=code-1&state={}", get("state"));
  assert_eq!(server.redirect(&back).await, "/");
  let signed_in = r#"{"user":{"name":"moderator","user_id":7}}"#;
  assert_eq!(server.get("/api/me", &[]).await, (200, signed_in.into()));
  assert!(server.dir.path().join("token.json").exists());

  server.kick.answer("/oauth/revoke", 200, "OK");
  let out = server.http.post(format!("{}/api/sign-out", server.base));
  assert_eq!(out.send().await.unwrap().status().as_u16(), 204);
  assert_eq!(server.get("/api/me", &[]).await, (200, signed_out.into()));
  assert!(!server.dir.path().join("token.json").exists());
}

#[tokio::test]
async fn says_why_it_didnt_sign_in() {
  let server = Server::start().await;
  let (status, page) = server
    .get("/oauth/callback?code=c&state=made-up", &[])
    .await;
  assert_eq!(status, 400);
  assert!(page.contains("Not signed in: this sign-in isn't one"));

  // What Kick says is shown as text.
  let to_kick = server.redirect("/sign-in").await;
  let url = Url::parse(&to_kick).unwrap();
  let (_, state) = url.query_pairs().find(|(k, _)| k == "state").unwrap();
  let back = format!("/oauth/callback?error=%3Cb%3Eno%3C/b%3E&state={state}");
  let (status, page) = server.get(&back, &[]).await;
  assert_eq!(status, 400);
  assert!(page.contains("Kick refused: &lt;b&gt;no&lt;/b&gt;"));
}

#[tokio::test]
async fn signing_out_needs_its_own_origin() {
  let server = Server::start().await;
  let out = server
    .http
    .post(format!("{}/api/sign-out", server.base))
    .header(ORIGIN, "https://example.com");
  assert_eq!(out.send().await.unwrap().status().as_u16(), 403);
}
