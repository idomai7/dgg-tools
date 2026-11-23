//! A local server standing in for Kick's sign-in and API: it answers each
//! path as the test says, and keeps what it was sent.

// Each test file uses its own part of it.
#![allow(dead_code)]

use {
  ::axum::{
    Router,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header::AUTHORIZATION},
  },
  ::std::{
    collections::HashMap,
    sync::{Arc, Mutex},
  },
  ::tokio::net::TcpListener,
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
