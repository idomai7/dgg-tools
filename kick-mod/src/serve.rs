//! The page, served on a loopback address.
//!
//! - `GET /`: the page.
//! - `GET /api/channels`: the channels, as `channels.json` has them now.
//!
//! There's no login yet, so it's served only on a loopback address (see
//! [`config`](crate::config)), and a request is refused (`403`) unless its
//! `Host` is the server's own, so a website the browser has open can't
//! reach it through DNS rebinding. When it has an `Origin`, that has to be
//! the server's own too, so a website can't act through it.

use {
  crate::channels,
  ::axum::{
    Json, Router,
    extract::{Request, State},
    http::{
      HeaderMap, StatusCode,
      header::{HOST, ORIGIN},
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
  },
  ::std::{io, net::SocketAddr, path::PathBuf, sync::Arc},
  ::tokio::net::TcpListener,
};

const PAGE: &str = include_str!("../page/index.html");

/// Serves the page on `listener` until it fails, with the channels in
/// `channels`.
pub async fn run(listener: TcpListener, channels: PathBuf) -> io::Result<()> {
  let router = router(listener.local_addr()?, channels);
  ::axum::serve(listener, router).await
}

/// The routes, served at `address`, with the channels in `channels`.
pub fn router(address: SocketAddr, channels: PathBuf) -> Router {
  Router::new()
    .route("/", get(page))
    .route("/api/channels", get(list_channels))
    .with_state(Arc::new(channels))
    .layer(middleware::from_fn_with_state(
      Arc::new(Allowed::at(address)),
      check,
    ))
}

/// The `Host`s and `Origin`s the server is reached by.
struct Allowed {
  hosts: Vec<String>,
  origins: Vec<String>,
}

impl Allowed {
  /// Its address, and on a loopback address, `localhost` too.
  fn at(address: SocketAddr) -> Self {
    let mut hosts = vec![address.to_string()];
    if address.ip().is_loopback() {
      hosts.push(format!("localhost:{}", address.port()));
    }
    let origins = hosts.iter().map(|host| format!("http://{host}")).collect();
    Self { hosts, origins }
  }
}

async fn check(
  State(allowed): State<Arc<Allowed>>,
  request: Request,
  next: Next,
) -> Response {
  if allowed.admits(request.headers()) {
    next.run(request).await
  } else {
    (StatusCode::FORBIDDEN, "not this server's").into_response()
  }
}

impl Allowed {
  /// Whether the `Host` is one of the server's, and the `Origin`, if any.
  fn admits(&self, headers: &HeaderMap) -> bool {
    let header = |name| headers.get(name)?.to_str().ok();
    let among = |value: &str, all: &[String]| {
      all.iter().any(|a| a.eq_ignore_ascii_case(value))
    };
    header(HOST).is_some_and(|host| among(host, &self.hosts))
      && header(ORIGIN).is_none_or(|origin| among(origin, &self.origins))
  }
}

async fn page() -> Html<&'static str> {
  Html(PAGE)
}

async fn list_channels(State(path): State<Arc<PathBuf>>) -> Response {
  match channels::load(&path) {
    Ok(channels) => Json(channels).into_response(),
    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
  }
}
