use {
  ::dgg_api::assets::fetch::{self, Error},
  ::tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
  },
};

const FLAIRS: &str = include_str!("fixtures/assets/flairs.json");

/// Serves one request with `status` and `body`; yields the request line.
async fn serve_once(
  status: &'static str,
  body: &'static str,
) -> (String, ::tokio::task::JoinHandle<String>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let cdn = format!("http://{}", listener.local_addr().unwrap());
  let server = ::tokio::spawn(async move {
    let (mut tcp, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.ends_with(b"\r\n\r\n") {
      let read = tcp.read(&mut buffer).await.unwrap();
      request.extend_from_slice(&buffer[..read]);
    }
    let response = format!(
      "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\
       Connection: close\r\n\r\n{body}",
      body.len()
    );
    tcp.write_all(response.as_bytes()).await.unwrap();
    let request = String::from_utf8(request).unwrap();
    request.lines().next().unwrap().to_owned()
  });
  (cdn, server)
}

#[::tokio::test]
async fn keeps_the_file_as_served() {
  let (cdn, server) = serve_once("200 OK", FLAIRS).await;
  let client = ::reqwest::Client::new();
  let fetched = fetch::flairs(&client, &cdn, Some("42")).await.unwrap();
  assert_eq!(fetched.raw, FLAIRS);
  assert_eq!(fetched.value.len(), 13);
  assert_eq!(
    server.await.unwrap(),
    "GET /flairs/flairs.json?_=42 HTTP/1.1"
  );
}

#[::tokio::test]
async fn keeps_what_does_not_decode() {
  let (cdn, _server) = serve_once("200 OK", "<html>").await;
  let client = ::reqwest::Client::new();
  match fetch::emotes(&client, &cdn, None).await {
    Err(Error::Decode { raw, .. }) => assert_eq!(raw, "<html>"),
    other => panic!("{other:?}"),
  }
}

#[::tokio::test]
async fn fails_on_an_error_status() {
  let (cdn, _server) = serve_once("404 Not Found", "").await;
  let client = ::reqwest::Client::new();
  let result = fetch::emotes(&client, &cdn, None).await;
  assert!(matches!(result, Err(Error::Http(_))), "{result:?}");
}

#[::tokio::test]
async fn stylesheets_come_as_served() {
  let css = ".emote.A {\n  width: 1px;\n}\n";
  let (cdn, server) = serve_once("200 OK", css).await;
  let client = ::reqwest::Client::new();
  assert_eq!(fetch::emotes_css(&client, &cdn, None).await.unwrap(), css);
  assert_eq!(server.await.unwrap(), "GET /emotes/emotes.css HTTP/1.1");
}
