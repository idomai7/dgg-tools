//! The page, served on a loopback address.
//!
//! - `GET /`: the page.
//! - `GET /sign-in`: off to Kick to sign in, which sends the moderator
//!   back to `GET /oauth/callback`, then to the page.
//! - `GET /api/me`: `{"user": {"user_id", "name"}}` for the signed-in
//!   moderator, as Kick has them now, or `{"user": null}`.
//! - `POST /api/sign-out`: signs them out.
//! - `GET /api/channels`: the channels, as `channels.json` has them now.
//! - `GET /api/events`: live chat, as server-sent events: each channel's
//!   status first, then every [`Update`] as JSON. An event `missed`, with
//!   a count, says the page fell behind and that many were dropped.
//!
//! There's no login yet, so it's served only on a loopback address (see
//! [`config`](crate::config)), and a request is refused (`403`) unless its
//! `Host` is the server's own, so a website the browser has open can't
//! reach it through DNS rebinding. When it has an `Origin`, that has to be
//! the server's own too, so a website can't act through it.

use {
  crate::{
    account::Account,
    channels,
    chat::{Chat, Update},
  },
  ::axum::{
    Json, Router,
    extract::{RawQuery, Request, State},
    http::{
      HeaderMap, StatusCode,
      header::{HOST, ORIGIN},
    },
    middleware::{self, Next},
    response::{
      Html, IntoResponse, Redirect, Response,
      sse::{self, KeepAlive, Sse},
    },
    routing::{get, post},
  },
  ::futures_util::{Stream, StreamExt, stream},
  ::serde_json::json,
  ::std::{convert::Infallible, io, net::SocketAddr, path::PathBuf, sync::Arc},
  ::tokio::{net::TcpListener, sync::broadcast::error::RecvError},
};

const PAGE: &str = include_str!("../page/index.html");

/// What the routes share.
struct Shared {
  channels: PathBuf,
  account: Account,
  chat: Chat,
}

/// Serves the page on `listener` until it fails, with the channels in
/// `channels`, and their chat.
pub async fn run(
  listener: TcpListener,
  channels: PathBuf,
  account: Account,
  chat: Chat,
) -> io::Result<()> {
  let router = router(listener.local_addr()?, channels, account, chat);
  ::axum::serve(listener, router).await
}

/// The routes, served at `address`, with the channels in `channels`, and
/// their chat.
pub fn router(
  address: SocketAddr,
  channels: PathBuf,
  account: Account,
  chat: Chat,
) -> Router {
  Router::new()
    .route("/", get(page))
    .route("/sign-in", get(sign_in))
    .route("/oauth/callback", get(callback))
    .route("/api/me", get(me))
    .route("/api/sign-out", post(sign_out))
    .route("/api/channels", get(list_channels))
    .route("/api/events", get(events))
    .with_state(Arc::new(Shared {
      channels,
      account,
      chat,
    }))
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

async fn page() -> Html<&'static str> {
  Html(PAGE)
}

async fn sign_in(State(shared): State<Arc<Shared>>) -> Redirect {
  Redirect::to(&shared.account.start())
}

async fn callback(
  State(shared): State<Arc<Shared>>,
  RawQuery(query): RawQuery,
) -> Response {
  match shared.account.finish(&query.unwrap_or_default()).await {
    Ok(_) => Redirect::to("/").into_response(),
    Err(e) => {
      let page = format!(
        "<!doctype html><title>kick-mod</title><p>Not signed in: {}</p>\
         <p><a href=\"/\">Back</a></p>",
        escape(&e)
      );
      (StatusCode::BAD_REQUEST, Html(page)).into_response()
    }
  }
}

async fn me(State(shared): State<Arc<Shared>>) -> Response {
  match shared.account.me().await {
    Ok(user) => {
      let user = user.map(|u| json!({"user_id": u.user_id, "name": u.name}));
      Json(json!({ "user": user })).into_response()
    }
    Err(e) => (StatusCode::BAD_GATEWAY, format!("Kick: {e}")).into_response(),
  }
}

async fn sign_out(State(shared): State<Arc<Shared>>) -> StatusCode {
  shared.account.sign_out().await;
  StatusCode::NO_CONTENT
}

async fn list_channels(State(shared): State<Arc<Shared>>) -> Response {
  match channels::load(&shared.channels) {
    Ok(channels) => Json(channels).into_response(),
    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
  }
}

async fn events(
  State(shared): State<Arc<Shared>>,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
  let (now, later) = shared.chat.watch();
  let later = stream::unfold(later, |mut later| async move {
    let event = match later.recv().await {
      Ok(update) => update_event(&update),
      Err(RecvError::Lagged(missed)) => sse::Event::default()
        .event("missed")
        .data(missed.to_string()),
      // Only when the chat has gone, with the server.
      Err(RecvError::Closed) => return None,
    };
    Some((event, later))
  });
  let events =
    stream::iter(now.into_iter().map(|update| update_event(&update)))
      .chain(later)
      .map(Ok);
  Sse::new(events).keep_alive(KeepAlive::default())
}

fn update_event(update: &Update) -> sse::Event {
  let json = ::serde_json::to_string(update).expect("updates serialize");
  sse::Event::default().data(json)
}

/// `text`, safe in HTML.
fn escape(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}
