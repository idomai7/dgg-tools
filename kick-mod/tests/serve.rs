//! The page's server, on a port of its own. Slugs and ids are made up.

use {
  ::kick_mod::serve,
  ::reqwest::header::{HOST, ORIGIN},
  ::std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
  },
  ::tokio::net::TcpListener,
};

/// The server, with its channels file, removed when the test ends.
struct Server {
  base: String,
  port: u16,
  dir: PathBuf,
}

impl Server {
  async fn start() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("kick-mod-serve-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    ::tokio::spawn(serve::run(listener, dir.join("channels.json")));
    Self {
      base: format!("http://127.0.0.1:{port}"),
      port,
      dir,
    }
  }

  async fn get(&self, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut request =
      ::reqwest::Client::new().get(format!("{}{path}", self.base));
    for &(name, value) in headers {
      request = request.header(name, value);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
  }
}

impl Drop for Server {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.dir);
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
    server.dir.join("channels.json"),
    r#"{"one": {"broadcaster_user_id": 1, "chatroom_id": null}}"#,
  )
  .unwrap();
  let (status, body) = server.get("/api/channels", &[]).await;
  assert_eq!(status, 200);
  assert_eq!(
    ::serde_json::from_str::<::serde_json::Value>(&body).unwrap(),
    ::serde_json::json!({"one": {"broadcaster_user_id": 1, "chatroom_id": null}})
  );
  fs::write(server.dir.join("channels.json"), "{ not json").unwrap();
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
