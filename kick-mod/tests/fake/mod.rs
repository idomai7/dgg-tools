//! Local servers standing in for Kick's: its sign-in and API, answering
//! each path as the test says and keeping what it was sent, and its
//! Centrifugo.

// Each test file uses its own part of it.
#![allow(dead_code)]

use {
  ::axum::{
    Router,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header::AUTHORIZATION},
  },
  ::futures_util::{SinkExt, StreamExt},
  ::kick_api::realtime::{Options, Setup},
  ::serde_json::{Value, json},
  ::std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
  },
  ::tokio::net::{TcpListener, TcpStream},
  ::tokio_tungstenite::{WebSocketStream, tungstenite::Message},
};

#[derive(Clone, Debug)]
pub struct Request {
  pub method: String,
  pub path: String,
  pub query: String,
  pub bearer: Option<String>,
  pub body: String,
}

#[derive(Default)]
struct Kept {
  answers: HashMap<String, (u16, String)>,
  requests: Vec<Request>,
}

pub struct Kick {
  /// `http://127.0.0.1:<port>`.
  pub base: String,
  kept: Arc<Mutex<Kept>>,
}

impl Kick {
  /// Serves until the test ends. Paths it hasn't been told of are `404`.
  pub async fn start() -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let kept = Arc::new(Mutex::new(Kept::default()));
    let router = Router::new().fallback(answer).with_state(kept.clone());
    ::tokio::spawn(async move { ::axum::serve(listener, router).await });
    Self { base, kept }
  }

  /// Answers `path` with `status` and `body` from now on.
  pub fn answer(&self, path: &str, status: u16, body: &str) {
    let answer = (status, body.to_owned());
    self
      .kept
      .lock()
      .unwrap()
      .answers
      .insert(path.into(), answer);
  }

  pub fn requests(&self) -> Vec<Request> {
    self.kept.lock().unwrap().requests.clone()
  }

  pub fn paths(&self) -> Vec<String> {
    self.requests().into_iter().map(|r| r.path).collect()
  }
}

async fn answer(
  State(kept): State<Arc<Mutex<Kept>>>,
  method: Method,
  uri: Uri,
  headers: HeaderMap,
  body: String,
) -> (StatusCode, String) {
  let bearer = headers
    .get(AUTHORIZATION)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.strip_prefix("Bearer "))
    .map(str::to_owned);
  let mut kept = kept.lock().unwrap();
  kept.requests.push(Request {
    method: method.to_string(),
    path: uri.path().into(),
    query: uri.query().unwrap_or_default().into(),
    bearer,
    body,
  });
  let (status, body) = kept
    .answers
    .get(uri.path())
    .cloned()
    .unwrap_or((404, r#"{"message":"Not found"}"#.into()));
  (StatusCode::from_u16(status).unwrap(), body)
}

pub type Ws = WebSocketStream<TcpStream>;

/// A local server standing in for Kick's Centrifugo, for one connection
/// at a time.
pub struct Centrifugo {
  /// `ws://127.0.0.1:<port>`.
  pub url: String,
  listener: TcpListener,
}

impl Centrifugo {
  pub async fn start() -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    Self { url, listener }
  }

  /// The client's options, to connect here.
  pub fn options(&self) -> Options {
    Options {
      setup: Setup::Fixed {
        endpoint: self.url.clone(),
        token: "t0k3n".into(),
      },
      min_backoff: Duration::from_millis(10),
      ..Default::default()
    }
  }

  /// Accepts a connection, answers its connect, and its subscribe to
  /// `channel`.
  pub async fn accept(&self, channel: &str) -> Ws {
    let (tcp, _) = self.listener.accept().await.unwrap();
    let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
    assert!(receive(&mut ws).await.contains(r#""connect""#));
    send(&mut ws, r#"{"id":1,"connect":{"client":"c1","ping":25}}"#).await;
    let subscribe =
      format!(r#"{{"id":2,"subscribe":{{"channel":"{channel}"}}}}"#);
    assert_eq!(receive(&mut ws).await, subscribe);
    send(&mut ws, r#"{"id":2,"subscribe":{}}"#).await;
    ws
  }
}

/// Publishes Kick's event `event` (without `App\Events\`), with `data`.
pub async fn publish(ws: &mut Ws, channel: &str, event: &str, data: Value) {
  let push = json!({
    "push": {
      "channel": channel,
      "pub": {
        "data": {
          "event": format!(r"App\Events\{event}"),
          "data": data.to_string(),
        },
      },
    },
  });
  send(ws, &push.to_string()).await;
}

async fn send(ws: &mut Ws, text: &str) {
  ws.send(Message::text(text)).await.unwrap();
}

async fn receive(ws: &mut Ws) -> String {
  match ws.next().await {
    Some(Ok(Message::Text(text))) => text.as_str().to_owned(),
    other => panic!("expected text, got {other:?}"),
  }
}
