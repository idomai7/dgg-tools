//! Serve: this host's journal over HTTP, for other hosts.
//!
//! It carries capture handoff and journal sync between hosts:
//!
//! - `PUT /control/<instance>`: an instance on another host pushes its
//!   status ([`control::push_body`]); it's written to this host's control
//!   directory with its times shifted onto this host's clock, so local
//!   instances can hand off to it.
//! - `DELETE /control/<instance>`: it's stopping.
//! - `GET /control`: `now <unix ms>`, a blank line, then every status here,
//!   each followed by a blank line.
//! - `GET /manifest`: every live journal file and backfill file, and its
//!   size ([`manifest`]).
//! - `GET /files/live/<day>/<instance>.journal` (or `/files/backfill/live/
//!   ...`): a file's bytes; with `Range: bytes=<from>-`, from that offset on
//!   (`206`). Likewise `/files/archive/<YYYY>/<MM>/<day>.journal.zst` (or
//!   `/files/backfill/archive/...`), for archives.
//!
//! Every request needs `Authorization: Bearer <token>`, with one of two
//! [`Tokens`]: the read token reads (enough for `sync`); only the control
//! token may push or remove a status, since a pushed status can make every
//! capture here retire. Tokens travel in the clear over plain HTTP, so
//! reach `serve` across hosts through something private (a VPN, an SSH
//! tunnel) or a TLS proxy.
//!
//! At most [`CONCURRENT`] requests are served at once (a file counts until
//! it's all sent); more get `503`. Files are sent as they're read, so a
//! large one doesn't sit in memory.

use {
  crate::{
    capture::now_ms,
    control::{self, Status},
    manifest::{self, archive_path, valid_live_path},
  },
  ::axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{
      HeaderMap, HeaderName, StatusCode,
      header::{AUTHORIZATION, CONTENT_LENGTH, RANGE},
    },
    routing::{get, put},
  },
  ::futures_util::stream::unfold,
  ::logger_journal::record::v0::valid_instance,
  ::std::{io::SeekFrom, path::PathBuf, sync::Arc},
  ::tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::{OwnedSemaphorePermit, Semaphore},
  },
};

/// Requests served at once.
pub const CONCURRENT: usize = 8;

/// The shortest token [`Tokens::check`] accepts.
pub const MIN_TOKEN: usize = 32;

/// What clients must send.
#[derive(Clone, Debug)]
pub struct Tokens {
  /// For reading: the manifest, files and statuses.
  pub read: String,
  /// For pushing and removing statuses too. Without it, nothing can.
  pub control: Option<String>,
}

impl Tokens {
  /// Why these tokens aren't fit to serve with, if they aren't: each must be
  /// [`MIN_TOKEN`] characters or more, and they must differ.
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
    Ok(())
  }
}

struct Shared {
  journal_root: PathBuf,
  control: PathBuf,
  tokens: Tokens,
  serving: Arc<Semaphore>,
}

/// What a request may do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
  Read,
  Control,
}

/// The routes, serving the journal at `journal_root`.
///
/// # Panics
///
/// If a token is empty.
pub fn router(journal_root: PathBuf, tokens: Tokens) -> Router {
  assert!(
    !tokens.read.is_empty()
      && tokens.control.as_ref().is_none_or(|t| !t.is_empty()),
    "serve needs non-empty tokens"
  );
  let shared = Arc::new(Shared {
    control: control::dir(&journal_root),
    journal_root,
    tokens,
    serving: Arc::new(Semaphore::new(CONCURRENT)),
  });
  Router::new()
    .route("/control", get(list))
    .route("/control/{instance}", put(push).delete(remove))
    .route("/manifest", get(manifest))
    .route("/files/{*path}", get(file))
    .with_state(shared)
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
  // Both compared, whichever matches.
  let control = shared
    .tokens
    .control
    .as_ref()
    .is_some_and(|control| same(given, control.as_bytes()));
  let read = same(given, shared.tokens.read.as_bytes());
  match (control, read) {
    (true, _) => Some(Access::Control),
    (_, true) => Some(Access::Read),
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
    Some(Access::Read) if needed == Access::Control => {
      return Err(StatusCode::FORBIDDEN);
    }
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
  match control::write(&shared.control, &status) {
    Ok(()) => StatusCode::NO_CONTENT,
    Err(e) => {
      eprintln!("control: {e}");
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
  match control::remove(&shared.control, &instance) {
    Ok(()) => StatusCode::NO_CONTENT,
    Err(e) => {
      eprintln!("control: {e}");
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

async fn manifest(
  State(shared): State<Arc<Shared>>,
  headers: HeaderMap,
) -> Result<String, StatusCode> {
  let _permit = admit(&shared, &headers, Access::Read)?;
  let files = manifest::list(&shared.journal_root)
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  Ok(manifest::encode(&files))
}

/// A live file's or archive's bytes, from the `Range: bytes=<from>-` offset
/// if given, as far as the file went when it was opened.
async fn file(
  State(shared): State<Arc<Shared>>,
  Path(path): Path<String>,
  headers: HeaderMap,
) -> Result<(StatusCode, [(HeaderName, u64); 1], Body), StatusCode> {
  let permit = admit(&shared, &headers, Access::Read)?;
  if !valid_live_path(&path) && archive_path(&path).is_none() {
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
  let mut opened = ::tokio::fs::File::open(shared.journal_root.join(&path))
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
