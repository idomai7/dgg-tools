//! Serve: this host's journals over HTTP, for other hosts. Each service's
//! routes start with its name and its API's version, `/<service>/<api>`
//! (see [`Service::API`]): `v0` for now, which may change incompatibly
//! from one build to the next until a stable `v1` is decided on. A
//! service's paths are relative to its directory.
//!
//! It carries journal sync between hosts, for every service:
//!
//! - `GET /<service>/<api>/manifest`: every live file and archive of the
//!   service's, and its size ([`manifest`]).
//! - `GET /<service>/<api>/files/<path>`: a file's bytes, `<path>` as the
//!   manifest gives it (see [`valid_path`]); with `Range: bytes=<from>-`,
//!   from that offset on (`206`).
//!
//! And dgg's capture handoff between hosts:
//!
//! - `PUT /dgg/v0/control/<instance>`: an instance on another host pushes its
//!   status ([`control::push_body`]); it's written to this host's control
//!   directory with its times shifted onto this host's clock, so local
//!   instances can hand off to it. Also to the one dgg's captures used
//!   before dgg's journals had a directory of their own, while it's there,
//!   so a capture from before hands off to it too.
//! - `DELETE /dgg/v0/control/<instance>`: it's stopping.
//! - `GET /dgg/v0/control`: `now <unix ms>`, a blank line, then every status
//!   here, each followed by a blank line.
//!
//! For kick-mod, Kick's history and live chat, under `/kick/v0/channels/`,
//! so no channel's slug can be mistaken for another route:
//!
//! - `GET /kick/v0/channels/<slug>/users/<id>`: a user's history in the
//!   channel, as JSON (see [`history`](crate::kick::history)), from Kick's
//!   index (the data directory's, beside the journal) and what it hasn't
//!   reached yet.
//! - `GET /kick/v0/channels/<slug>/events`: the channel's live chat as server-sent
//!   events, by following its live files from now on (see
//!   [`tail`](crate::kick::tail)), looking every [`LOOK`]. Each event's
//!   data is JSON, with `received`, `instance` and `connection`, and:
//!   `open` an `endpoint`; `close` a `reason`; `event` a Kick `event` and
//!   its `data`, both as Kick sent them; `history` a `page` of chat history
//!   a gap was filled with, as Kick sent it. An `error` ends the stream.
//!
//! Every request needs `Authorization: Bearer <token>`, with one of three
//! [`Tokens`]: the query token reads only Kick's history and live chat; the
//! read token reads everything (enough for `sync`); only the control token
//! may push or remove a status, since a pushed status can make every
//! capture here retire. Across hosts, [`run`] it over TLS, with a
//! certificate from the mesh's CA ([`crate::cert`]): over plain HTTP, tokens
//! travel in the clear.
//!
//! At most [`CONCURRENT`] requests are served at once (a file counts until
//! it's all sent); more get `503`. Files are sent as they're read, so a
//! large one doesn't sit in memory. Live chat streams count apart, at most
//! [`STREAMS`] at once.

use {
  crate::{
    control::{self, Status},
    dgg::Dgg,
    kick::{
      self, Kick,
      channels::valid_slug,
      tail::{Tail, What, Written},
    },
    manifest::{self, valid_path},
    service::{Service, locate, routes},
  },
  ::axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{
      HeaderMap, HeaderName, StatusCode,
      header::{AUTHORIZATION, CONTENT_LENGTH, RANGE},
    },
    response::sse::{self, KeepAlive, Sse},
    routing::{get, put},
  },
  ::common::time::now_ms,
  ::common::warn,
  ::futures_util::{Stream, stream::unfold},
  ::logger_journal::{layout::channel_root, record::v0::valid_instance},
  ::rustls::ServerConfig,
  ::serde_json::{Value, json},
  ::std::{
    collections::VecDeque,
    convert::Infallible,
    io::{self, SeekFrom},
    path::PathBuf,
    sync::Arc,
    time::Duration,
  },
  ::tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore},
    task::spawn_blocking,
  },
};

/// Requests served at once.
pub const CONCURRENT: usize = 8;

/// Live chat streams served at once.
pub const STREAMS: usize = 16;

/// How often a live chat stream looks for what's been written.
pub const LOOK: Duration = Duration::from_millis(250);

/// The shortest token [`Tokens::check`] accepts.
pub const MIN_TOKEN: usize = 32;

/// What clients must send.
#[derive(Clone, Debug)]
pub struct Tokens {
  /// For reading: the manifest, files and statuses.
  pub read: String,
  /// For pushing and removing statuses too. Without it, nothing can.
  pub control: Option<String>,
  /// For Kick's history and live chat only, for kick-mod. Without it, only
  /// the others read them.
  pub query: Option<String>,
}

impl Tokens {
  /// Why these tokens aren't fit to serve with, if they aren't: each must be
  /// [`MIN_TOKEN`] characters or more, and no two may be the same.
  pub fn check(&self) -> Result<(), String> {
    let short = |name: &str, token: &str| {
      (token.chars().count() < MIN_TOKEN).then(|| {
        format!("the {name} token must be {MIN_TOKEN} characters or more")
      })
    };
    if let Some(why) = short("read", &self.read) {
      return Err(why);
    }
    if let Some(control) = &self.control {
      if let Some(why) = short("control", control) {
        return Err(why);
      }
      if *control == self.read {
        return Err("the read and control tokens must differ".into());
      }
    }
    if let Some(query) = &self.query {
      if let Some(why) = short("query", query) {
        return Err(why);
      }
      if *query == self.read || Some(query) == self.control.as_ref() {
        return Err("the query token must differ from the others".into());
      }
    }
    Ok(())
  }
}

struct Shared {
  /// The journal directory, holding each service's.
  journal: PathBuf,
  /// dgg's capture statuses.
  control: PathBuf,
  /// Where they were before dgg's journals had a directory of their own.
  legacy_control: PathBuf,
  /// Kick's index, in the data directory.
  kick_index: PathBuf,
  tokens: Tokens,
  serving: Arc<Semaphore>,
  streams: Arc<Semaphore>,
}

/// What a request may do, each more than the one before.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Access {
  Query,
  Read,
  Control,
}

/// The routes, serving the journal directory `journal`, and Kick's index
/// beside it, in its parent, the data directory.
///
/// # Panics
///
/// If a token is empty.
pub fn router(journal: PathBuf, tokens: Tokens) -> Router {
  assert!(
    !tokens.read.is_empty()
      && tokens.control.as_ref().is_none_or(|t| !t.is_empty())
      && tokens.query.as_ref().is_none_or(|t| !t.is_empty()),
    "serve needs non-empty tokens"
  );
  let data = journal.parent().unwrap_or(&journal);
  let shared = Arc::new(Shared {
    control: control::dir(&Dgg::dir(&journal)),
    legacy_control: control::dir(&journal),
    kick_index: data.join(Kick::INDEX),
    journal,
    tokens,
    serving: Arc::new(Semaphore::new(CONCURRENT)),
    streams: Arc::new(Semaphore::new(STREAMS)),
  });
  let (dgg, kick) = (routes::<Dgg>(), routes::<Kick>());
  Router::new()
    .route(&format!("{dgg}/control"), get(list))
    .route(
      &format!("{dgg}/control/{{instance}}"),
      put(push).delete(remove),
    )
    .merge(journals::<Dgg>())
    .merge(journals::<Kick>())
    .route(
      &format!("{kick}/channels/{{slug}}/users/{{id}}"),
      get(kick_user),
    )
    .route(
      &format!("{kick}/channels/{{slug}}/events"),
      get(kick_events),
    )
    .with_state(shared)
}

/// `S`'s routes for sync: its manifest and its files.
fn journals<S: Service>() -> Router<Arc<Shared>> {
  let at = routes::<S>();
  Router::new()
    .route(&format!("{at}/manifest"), get(listing::<S>))
    .route(&format!("{at}/files/{{*path}}"), get(file::<S>))
}

/// Serves `app` on `listener` until that fails: over TLS with `tls`, else
/// plain HTTP.
pub async fn run(
  listener: TcpListener,
  app: Router,
  tls: Option<ServerConfig>,
) -> io::Result<()> {
  match tls {
    None => ::axum::serve(listener, app).await,
    Some(tls) => {
      let tls =
        ::axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls));
      ::axum_server::tls_rustls::from_tcp_rustls(listener.into_std()?, tls)?
        .serve(app.into_make_service())
        .await
    }
  }
}

/// Whether `given` is `expected`, compared in constant time.
fn same(given: &[u8], expected: &[u8]) -> bool {
  given.len() == expected.len()
    && given
      .iter()
      .zip(expected)
      .fold(0, |difference, (a, b)| difference | (a ^ b))
      == 0
}

/// What the request's token allows, if anything.
fn access(shared: &Shared, headers: &HeaderMap) -> Option<Access> {
  let given = headers
    .get(AUTHORIZATION)?
    .to_str()
    .ok()?
    .strip_prefix("Bearer ")?
    .as_bytes();
  // Each compared, whichever matches.
  let matches = |token: Option<&String>| {
    token.is_some_and(|token| same(given, token.as_bytes()))
  };
  let control = matches(shared.tokens.control.as_ref());
  let read = matches(Some(&shared.tokens.read));
  let query = matches(shared.tokens.query.as_ref());
  match (control, read, query) {
    (true, _, _) => Some(Access::Control),
    (_, true, _) => Some(Access::Read),
    (_, _, true) => Some(Access::Query),
    _ => None,
  }
}

/// Lets the request through if its token allows `needed` and there's room:
/// a permit to hold while serving it.
fn admit(
  shared: &Shared,
  headers: &HeaderMap,
  needed: Access,
) -> Result<OwnedSemaphorePermit, StatusCode> {
  match access(shared, headers) {
    None => return Err(StatusCode::UNAUTHORIZED),
    Some(access) if access < needed => return Err(StatusCode::FORBIDDEN),
    Some(_) => {}
  }
  shared
    .serving
    .clone()
    .try_acquire_owned()
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

async fn push(
  State(shared): State<Arc<Shared>>,
  Path(instance): Path<String>,
  headers: HeaderMap,
  body: String,
) -> StatusCode {
  let _permit = match admit(&shared, &headers, Access::Control) {
    Ok(permit) => permit,
    Err(status) => return status,
  };
  let Some(status) = control::parse_push(&body, now_ms()) else {
    return StatusCode::BAD_REQUEST;
  };
  if status.instance != instance {
    return StatusCode::BAD_REQUEST;
  }
  if let Err(e) = control::write_if_there(&shared.legacy_control, &status) {
    warn!("control: {e}");
  }
  match control::write(&shared.control, &status) {
    Ok(()) => StatusCode::NO_CONTENT,
    Err(e) => {
      warn!("control: {e}");
      StatusCode::SERVICE_UNAVAILABLE
    }
  }
}

async fn remove(
  State(shared): State<Arc<Shared>>,
  Path(instance): Path<String>,
  headers: HeaderMap,
) -> StatusCode {
  let _permit = match admit(&shared, &headers, Access::Control) {
    Ok(permit) => permit,
    Err(status) => return status,
  };
  if !valid_instance(&instance) {
    return StatusCode::BAD_REQUEST;
  }
  if let Err(e) = control::remove(&shared.legacy_control, &instance) {
    warn!("control: {e}");
  }
  match control::remove(&shared.control, &instance) {
    Ok(()) => StatusCode::NO_CONTENT,
    Err(e) => {
      warn!("control: {e}");
      StatusCode::SERVICE_UNAVAILABLE
    }
  }
}

async fn list(
  State(shared): State<Arc<Shared>>,
  headers: HeaderMap,
) -> Result<String, StatusCode> {
  let _permit = admit(&shared, &headers, Access::Read)?;
  let statuses = control::read_all(&shared.control)
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  let mut body = format!("now {}\n\n", now_ms());
  for status in statuses {
    body += &Status::encode(&status);
    body += "\n";
  }
  Ok(body)
}

/// `S`'s manifest.
async fn listing<S: Service>(
  State(shared): State<Arc<Shared>>,
  headers: HeaderMap,
) -> Result<String, StatusCode> {
  let _permit = admit(&shared, &headers, Access::Read)?;
  let files = manifest::list::<S>(&S::dir(&shared.journal))
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  Ok(manifest::encode::<S>(&files))
}

/// One of `S`'s live files' or archives' bytes, from the `Range:
/// bytes=<from>-` offset if given, as far as the file went when it was
/// opened.
async fn file<S: Service>(
  State(shared): State<Arc<Shared>>,
  Path(path): Path<String>,
  headers: HeaderMap,
) -> Result<(StatusCode, [(HeaderName, u64); 1], Body), StatusCode> {
  let permit = admit(&shared, &headers, Access::Read)?;
  if !valid_path::<S>(&path) {
    return Err(StatusCode::BAD_REQUEST);
  }
  let from = match headers.get(RANGE) {
    None => None,
    Some(range) => Some(
      range
        .to_str()
        .ok()
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|from| from.parse::<u64>().ok())
        .ok_or(StatusCode::RANGE_NOT_SATISFIABLE)?,
    ),
  };
  let local = locate::<S>(&S::dir(&shared.journal), &path);
  let mut opened = ::tokio::fs::File::open(local)
    .await
    .map_err(|_| StatusCode::NOT_FOUND)?;
  let size = opened
    .metadata()
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .len();
  let from = from.unwrap_or(0);
  if from > size {
    return Err(StatusCode::RANGE_NOT_SATISFIABLE);
  }
  opened
    .seek(SeekFrom::Start(from))
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  let status = match headers.contains_key(RANGE) {
    true => StatusCode::PARTIAL_CONTENT,
    false => StatusCode::OK,
  };
  // The permit goes with the body, held until it's all sent.
  let chunks = unfold(
    (opened.take(size - from), permit),
    |(mut file, permit)| async move {
      let mut chunk = vec![0; 64 << 10];
      match file.read(&mut chunk).await {
        Ok(0) => None,
        Ok(read) => {
          chunk.truncate(read);
          Some((Ok(chunk), (file, permit)))
        }
        Err(e) => Some((Err(e), (file, permit))),
      }
    },
  );
  Ok((
    status,
    [(CONTENT_LENGTH, size - from)],
    Body::from_stream(chunks),
  ))
}

/// The journal root of Kick's channel `slug`, if it's one with a journal
/// here.
fn kick_channel(shared: &Shared, slug: &str) -> Result<PathBuf, StatusCode> {
  if !valid_slug(slug) {
    return Err(StatusCode::BAD_REQUEST);
  }
  let root = channel_root(&Kick::dir(&shared.journal), slug);
  match root.is_dir() {
    true => Ok(root),
    false => Err(StatusCode::NOT_FOUND),
  }
}

/// A user's history in a Kick channel.
async fn kick_user(
  State(shared): State<Arc<Shared>>,
  Path((slug, id)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
  let _permit = admit(&shared, &headers, Access::Query)?;
  let user_id: u64 = id.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
  kick_channel(&shared, &slug)?;
  let found = spawn_blocking(move || {
    kick::history::user(&shared.kick_index, &shared.journal, &slug, user_id)
  })
  .await
  .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  match found {
    Ok(history) => Ok(Json(history)),
    Err(e) => {
      warn!("kick history: {e}");
      Err(StatusCode::SERVICE_UNAVAILABLE)
    }
  }
}

/// A Kick channel's live chat, from now on.
async fn kick_events(
  State(shared): State<Arc<Shared>>,
  Path(slug): Path<String>,
  headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, StatusCode>
{
  // Held while the stream starts; the stream holds one of its own.
  let _permit = admit(&shared, &headers, Access::Query)?;
  let root = kick_channel(&shared, &slug)?;
  let permit = shared
    .streams
    .clone()
    .try_acquire_owned()
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  let tail = spawn_blocking(move || Tail::new(root))
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .map_err(|e| {
      warn!("kick {slug} events: {e}");
      StatusCode::SERVICE_UNAVAILABLE
    })?;
  // The permit goes with the stream, held until it ends. Without a tail,
  // the stream has ended.
  let state = (Some(tail), VecDeque::new(), permit);
  let events = unfold(state, |(mut tail, mut queue, permit)| async move {
    loop {
      if let Some(written) = queue.pop_front() {
        return Some((Ok(sse_event(written)), (tail, queue, permit)));
      }
      let mut following = tail.take()?;
      ::tokio::time::sleep(LOOK).await;
      let looked = spawn_blocking(move || {
        let found = following.read();
        (following, found)
      })
      .await;
      match looked {
        Ok((following, Ok(found))) => {
          tail = Some(following);
          queue.extend(found);
        }
        Ok((_, Err(e))) => {
          let error = sse::Event::default().event("error").data(e.to_string());
          return Some((Ok(error), (None, queue, permit)));
        }
        Err(_) => return None,
      }
    }
  });
  Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

/// What was written to a Kick channel's live files, as a server-sent event.
fn sse_event(written: Written) -> sse::Event {
  let (kind, mut data) = match written.what {
    What::Open { endpoint } => ("open", json!({"endpoint": endpoint})),
    What::Close { reason } => ("close", json!({"reason": reason})),
    What::Event { event, data } => {
      ("event", json!({"event": event, "data": data}))
    }
    What::History { page } => ("history", json!({"page": page})),
  };
  data["received"] = written.received.into();
  data["instance"] = written.instance.into();
  data["connection"] = written.connection.into();
  sse::Event::default().event(kind).data(data.to_string())
}
