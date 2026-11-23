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
//! - `GET /api/history?channel=<slug>&user=<id>`: the user's
//!   [history](History) in the channel.
//! - `POST /api/actions`: an [action](Asked), as the signed-in moderator,
//!   logged in the [mod log](modlog). `204` when Kick did it; otherwise why
//!   not, as text.
//!
//! There's no login yet, so it's served only on a loopback address (see
//! [`config`](crate::config)), and a request is refused (`403`) unless its
//! `Host` is the server's own, so a website the browser has open can't
//! reach it through DNS rebinding. When it has an `Origin`, that has to be
//! the server's own too, so a website can't act through it.

use {
  crate::{
    account::Account,
    actions::Asked,
    channels,
    chat::{Chat, Update},
    history::{self, History},
    modlog::{self, Answer, Entry, Moderator},
  },
  ::axum::{
    Json, Router,
    extract::{Query, RawQuery, Request, State},
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
  ::common::fs::MissingOk,
  ::futures_util::{Stream, StreamExt, stream},
  ::serde::Deserialize,
  ::serde_json::json,
  ::std::{
    convert::Infallible,
    fs, io,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
  },
  ::tokio::{net::TcpListener, sync::broadcast::error::RecvError},
};

const PAGE: &str = include_str!("../page/index.html");

/// What's served.
pub struct Served {
  /// `channels.json`.
  pub channels: PathBuf,
  /// Kick's journal root, a directory per channel.
  pub journal: PathBuf,
  /// `modlog.jsonl`.
  pub modlog: PathBuf,
  pub account: Account,
  /// The channels' chat.
  pub chat: Chat,
}

/// What the routes share.
struct Shared {
  served: Served,
  /// Held while appending to the mod log.
  logging: Mutex<()>,
}

/// Serves the page on `listener` until it fails.
pub async fn run(listener: TcpListener, served: Served) -> io::Result<()> {
  let router = router(listener.local_addr()?, served);
  ::axum::serve(listener, router).await
}

/// The routes, served at `address`.
pub fn router(address: SocketAddr, served: Served) -> Router {
  Router::new()
    .route("/", get(page))
    .route("/sign-in", get(sign_in))
    .route("/oauth/callback", get(callback))
    .route("/api/me", get(me))
    .route("/api/sign-out", post(sign_out))
    .route("/api/channels", get(list_channels))
    .route("/api/events", get(events))
    .route("/api/history", get(lookup))
    .route("/api/actions", post(act))
    .with_state(Arc::new(Shared {
      served,
      logging: Mutex::new(()),
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
  Redirect::to(&shared.served.account.start())
}

async fn callback(
  State(shared): State<Arc<Shared>>,
  RawQuery(query): RawQuery,
) -> Response {
  match shared
    .served
    .account
    .finish(&query.unwrap_or_default())
    .await
  {
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
  match shared.served.account.me().await {
    Ok(user) => {
      let user = user.map(|u| json!({"user_id": u.user_id, "name": u.name}));
      Json(json!({ "user": user })).into_response()
    }
    Err(e) => (StatusCode::BAD_GATEWAY, format!("Kick: {e}")).into_response(),
  }
}

async fn sign_out(State(shared): State<Arc<Shared>>) -> StatusCode {
  shared.served.account.sign_out().await;
  StatusCode::NO_CONTENT
}

async fn list_channels(State(shared): State<Arc<Shared>>) -> Response {
  match channels::load(&shared.served.channels) {
    Ok(channels) => Json(channels).into_response(),
    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
  }
}

async fn events(
  State(shared): State<Arc<Shared>>,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
  let (now, later) = shared.served.chat.watch();
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

async fn act(
  State(shared): State<Arc<Shared>>,
  Json(asked): Json<Asked>,
) -> Response {
  let served = &shared.served;
  let asked = match asked.check() {
    Ok(asked) => asked,
    Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
  };
  let known = match channels::load(&served.channels) {
    Ok(known) => known,
    Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
  };
  let Some(channel) = known.get(&asked.channel) else {
    let unknown = format!("no channel {} in channels.json", asked.channel);
    return (StatusCode::NOT_FOUND, unknown).into_response();
  };
  let (api, moderator) = match served.account.moderator().await {
    Ok(Some(signed_in)) => signed_in,
    Ok(None) => {
      let out = "sign in to moderate";
      return (StatusCode::UNAUTHORIZED, out).into_response();
    }
    Err(e) => {
      let unknown = format!("Kick didn't say who's signed in: {e}");
      return (StatusCode::BAD_GATEWAY, unknown).into_response();
    }
  };
  let broadcaster_user_id = channel.broadcaster_user_id;
  let answer = match asked.perform(&api, broadcaster_user_id).await {
    Ok(()) => Answer::Done,
    Err(e) => Answer::Refused {
      status: e.status(),
      error: e.message().unwrap_or_else(|| e.to_string()),
    },
  };
  let entry = Entry {
    at: ::common::time::now_ms(),
    by: Moderator {
      user_id: moderator.user_id,
      name: moderator.name,
    },
    broadcaster_user_id,
    asked,
    answer,
  };
  let logged = {
    let _logging = shared.logging.lock().unwrap();
    modlog::append(&served.modlog, &entry)
  };
  let unlogged =
    |e: io::Error| format!("{}: not logged: {e}", served.modlog.display());
  match (entry.answer, logged) {
    (Answer::Done, Ok(())) => StatusCode::NO_CONTENT.into_response(),
    (Answer::Done, Err(e)) => {
      let done = format!("done, but {}", unlogged(e));
      (StatusCode::INTERNAL_SERVER_ERROR, done).into_response()
    }
    (Answer::Refused { status, error }, logged) => {
      let status = status.map(|s| format!(" {s}")).unwrap_or_default();
      let mut refused = format!("Kick refused{status}: {error}");
      if let Err(e) = logged {
        refused = format!("{refused}; and {}", unlogged(e));
      }
      (StatusCode::BAD_GATEWAY, refused).into_response()
    }
  }
}

/// Whose history, in which channel.
#[derive(Deserialize)]
struct Lookup {
  channel: String,
  user: u64,
}

async fn lookup(
  State(shared): State<Arc<Shared>>,
  Query(lookup): Query<Lookup>,
) -> Response {
  let read = ::tokio::task::spawn_blocking(move || {
    let served = &shared.served;
    let modlog = {
      // So a line being appended isn't read half-written.
      let _logging = shared.logging.lock().unwrap();
      fs::read_to_string(&served.modlog).missing_ok()
    };
    let modlog = modlog
      .map_err(|e| format!("{}: {e}", served.modlog.display()))?
      .unwrap_or_default();
    history::read(&served.journal, &modlog, &lookup.channel, lookup.user)
  });
  match read.await.expect("reading history doesn't panic") {
    Ok(history) => Json::<History>(history).into_response(),
    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
  }
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
