//! A local HTTP server standing in for rustlesearch.dev: it answers each
//! request with what the test says, and keeps what it was sent.

use {
  ::reqwest::Url,
  ::std::{
    collections::HashMap,
    sync::{Arc, Mutex},
  },
  ::tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
  },
};

/// A request, as the server read it. Requests have no body.
#[derive(Clone, Debug)]
pub struct Request {
  pub path: String,
  pub query: Vec<(String, String)>,
  /// Lowercased names.
  pub headers: HashMap<String, String>,
}

/// An answer: its status, extra headers and body.
pub type Reply = (u16, Vec<(&'static str, String)>, String);

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
}

/// Serves until the test ends, answering each request with `answer`.
pub async fn serve(
  answer: impl Fn(&Request) -> Reply + Send + Sync + 'static,
) -> Server {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let requests = Arc::new(Mutex::new(Vec::new()));
  let answer = Arc::new(answer);
  let kept = requests.clone();
  ::tokio::spawn(async move {
    loop {
      let (mut socket, _) = listener.accept().await.unwrap();
      let (answer, kept) = (answer.clone(), kept.clone());
      ::tokio::spawn(async move {
        let mut read = Vec::new();
        while !read.windows(4).any(|w| w == b"\r\n\r\n") {
          let mut chunk = [0; 4096];
          let n = socket.read(&mut chunk).await.unwrap();
          assert_ne!(n, 0, "the client closed mid-request");
          read.extend_from_slice(&chunk[..n]);
        }
        let request = parse(&String::from_utf8(read).unwrap());
        let (status, headers, body) = answer(&request);
        kept.lock().unwrap().push(request);
        let reason = if status < 400 { "OK" } else { "Error" };
        let mut head = format!(
          "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\n\
           connection: close\r\n",
          body.len()
        );
        for (name, value) in headers {
          head += &format!("{name}: {value}\r\n");
        }
        head += "\r\n";
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
      });
    }
  });
  Server { base, requests }
}

fn parse(text: &str) -> Request {
  let mut lines = text.split("\r\n");
  let target = lines.next().unwrap().split(' ').nth(1).unwrap();
  let headers = lines
    .filter_map(|line| line.split_once(": "))
    .map(|(name, value)| (name.to_lowercase(), value.to_owned()))
    .collect();
  let url = Url::parse(&format!("http://server{target}")).unwrap();
  Request {
    path: url.path().into(),
    query: url.query_pairs().into_owned().collect(),
    headers,
  }
}
