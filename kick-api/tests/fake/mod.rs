//! A local HTTP server standing in for Kick's: it answers each request
//! with what the test says, and keeps what it was sent.

// Each test file uses its own part of it.
#![allow(dead_code)]

use {
  ::reqwest::Url,
  ::std::{
    collections::HashMap,
    sync::{Arc, Mutex},
  },
  ::tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
  },
};

/// A request, as the server read it.
#[derive(Clone, Debug)]
pub struct Request {
  pub method: String,
  pub path: String,
  pub query: Vec<(String, String)>,
  /// Lowercased names.
  pub headers: HashMap<String, String>,
  pub body: String,
}

impl Request {
  /// The body, as a form.
  pub fn form(&self) -> HashMap<String, String> {
    let url = Url::parse(&format!("http://server/?{}", self.body)).unwrap();
    url.query_pairs().into_owned().collect()
  }

  /// The body, as JSON.
  pub fn json(&self) -> ::serde_json::Value {
    ::serde_json::from_str(&self.body).unwrap()
  }

  pub fn bearer(&self) -> Option<&str> {
    self.headers.get("authorization")?.strip_prefix("Bearer ")
  }
}

pub struct Server {
  /// `http://127.0.0.1:<port>`.
  pub base: String,
  requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
  /// What it was sent, in order.
  pub fn requests(&self) -> Vec<Request> {
    self.requests.lock().unwrap().clone()
  }

  /// The paths it was sent, in order.
  pub fn paths(&self) -> Vec<String> {
    self.requests().into_iter().map(|r| r.path).collect()
  }
}

/// Serves until the test ends, answering each request with a status and
/// a body from `answer`.
pub async fn serve(
  answer: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static,
) -> Server {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let requests = Arc::new(Mutex::new(Vec::new()));
  let answer = Arc::new(answer);
  let kept = requests.clone();
  ::tokio::spawn(async move {
    loop {
      let (socket, _) = listener.accept().await.unwrap();
      let (answer, kept) = (answer.clone(), kept.clone());
      ::tokio::spawn(async move {
        let (mut socket, request) = read(socket).await;
        let (status, body) = answer(&request);
        kept.lock().unwrap().push(request);
        let reason = if status < 400 { "OK" } else { "Error" };
        let length = if status == 204 { 0 } else { body.len() };
        let head = format!(
          "HTTP/1.1 {status} {reason}\r\ncontent-length: {length}\r\n\
           connection: close\r\n\r\n"
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        if status != 204 {
          socket.write_all(body.as_bytes()).await.unwrap();
        }
      });
    }
  });
  Server { base, requests }
}

/// One request, read whole.
async fn read(mut socket: TcpStream) -> (TcpStream, Request) {
  let mut read = Vec::new();
  let head = loop {
    if let Some(end) = read.windows(4).position(|w| w == b"\r\n\r\n") {
      break end;
    }
    more(&mut socket, &mut read).await;
  };
  let text = String::from_utf8(read[..head].to_vec()).unwrap();
  let mut lines = text.split("\r\n");
  let mut start = lines.next().unwrap().split(' ');
  let (method, target) = (start.next().unwrap(), start.next().unwrap());
  let headers: HashMap<_, _> = lines
    .filter_map(|line| line.split_once(": "))
    .map(|(name, value)| (name.to_lowercase(), value.to_owned()))
    .collect();
  let length = headers
    .get("content-length")
    .map_or(0, |length| length.parse().unwrap());
  while read.len() < head + 4 + length {
    more(&mut socket, &mut read).await;
  }
  let url = Url::parse(&format!("http://server{target}")).unwrap();
  let request = Request {
    method: method.into(),
    path: url.path().into(),
    query: url.query_pairs().into_owned().collect(),
    headers,
    body: String::from_utf8(read[head + 4..].to_vec()).unwrap(),
  };
  (socket, request)
}

async fn more(socket: &mut TcpStream, read: &mut Vec<u8>) {
  let mut chunk = [0; 4096];
  let n = socket.read(&mut chunk).await.unwrap();
  assert_ne!(n, 0, "the client closed mid-request");
  read.extend_from_slice(&chunk[..n]);
}
