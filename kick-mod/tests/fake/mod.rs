//! A local server standing in for Kick's sign-in and API, answering each
//! path as the test says and keeping what it was sent, and for the
//! logger's `serve`: its channels' live chat streams, fed by the test, and
//! any other path as the test says.

// Each test file uses its own part of it.
#![allow(dead_code)]

use {
  ::axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{
      HeaderMap, Method, StatusCode, Uri,
      header::{AUTHORIZATION, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
    routing::get,
  },
  ::common::{config::Secret, time::now_ms},
  ::futures_util::stream,
  ::kick_mod::{config, logger},
  ::serde_json::{Value, json},
  ::std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
  },
  ::tokio::{net::TcpListener, sync::mpsc, time::sleep},
};

/// The logger's query token.
pub const QUERY_TOKEN: &str = "made-up-query-token";

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
  /// Each channel's open stream, by slug.
  streams: HashMap<String, mpsc::UnboundedSender<String>>,
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
    let router = Router::new()
      .route("/kick/v0/channels/{slug}/events", get(events))
      .fallback(answer)
      .with_state(kept.clone());
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

  /// A client of the logger here, with `token`, retrying soon.
  pub fn logger(&self, token: &str) -> logger::Client {
    let config = config::Logger {
      url: format!("{}/", self.base),
      token: Secret::new(token),
      ca: None,
    };
    let client = logger::Client::new(&config).unwrap();
    client.with_first_retry(Duration::from_millis(10))
  }

  /// Sends the logger's event `kind` with `data` on channel `slug`'s
  /// stream, once one's open.
  pub async fn send(&self, slug: &str, kind: &str, data: Value) {
    let event = format!("event: {kind}\ndata: {data}\n\n");
    for _ in 0..500 {
      if let Some(stream) = self.kept.lock().unwrap().streams.get(slug) {
        stream.send(event).unwrap();
        return;
      }
      sleep(Duration::from_millis(10)).await;
    }
    panic!("no stream opened");
  }

  /// Ends channel `slug`'s open stream.
  pub fn drop_stream(&self, slug: &str) {
    self.kept.lock().unwrap().streams.remove(slug).unwrap();
  }
}

/// What the logger passes on, with `more`.
pub fn sent(more: Value) -> Value {
  let mut sent =
    json!({"received": now_ms(), "instance": "capture-1", "connection": 1});
  let more = more.as_object().unwrap().clone();
  sent.as_object_mut().unwrap().extend(more);
  sent
}

/// Kick's event `name` (without `App\Events\`) with `data`, as the logger
/// passes it on.
pub fn kick_event(name: &str, data: Value) -> Value {
  let event = format!(r"App\Events\{name}");
  sent(json!({"event": event, "data": data.to_string()}))
}

async fn events(
  State(kept): State<Arc<Mutex<Kept>>>,
  Path(slug): Path<String>,
  headers: HeaderMap,
) -> Response {
  let bearer = format!("Bearer {QUERY_TOKEN}");
  if headers.get(AUTHORIZATION).unwrap().to_str().unwrap() != bearer {
    return StatusCode::UNAUTHORIZED.into_response();
  }
  let (sender, receiver) = mpsc::unbounded_channel::<String>();
  kept.lock().unwrap().streams.insert(slug, sender);
  let body = stream::unfold(receiver, |mut receiver| async move {
    let text = receiver.recv().await?;
    Some((Ok::<_, Infallible>(text), receiver))
  });
  (
    [(CONTENT_TYPE, "text/event-stream")],
    Body::from_stream(body),
  )
    .into_response()
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
