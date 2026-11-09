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
//!   size ([`manifest`](crate::manifest)).
//! - `GET /files/live/<day>/<instance>.journal` (or `/files/backfill/live/
//!   ...`): a file's bytes; with `Range: bytes=<from>-`, from that offset on
//!   (`206`). Likewise `/files/archive/<YYYY>/<MM>/<day>.journal.zst` (or
//!   `/files/backfill/archive/...`), for archives.
//!
//! Every request needs `Authorization: Bearer <token>`. The token travels
//! in the clear over plain HTTP, so reach `serve` across hosts through
//! something private (a VPN, an SSH tunnel) or a TLS proxy.

use {
  crate::{
    capture::now_ms,
    control::{self, Status},
    manifest::{self, archive_path, valid_live_path},
  },
  ::axum::{
    Router,
    extract::{Path, State},
    http::{
      HeaderMap, StatusCode,
      header::{AUTHORIZATION, RANGE},
    },
    routing::{get, put},
  },
  ::journal::record::v0::valid_instance,
  ::std::{io::SeekFrom, path::PathBuf, sync::Arc},
  ::tokio::io::{AsyncReadExt, AsyncSeekExt},
};

struct Shared {
  journal_root: PathBuf,
  control: PathBuf,
  token: String,
}

/// The routes, serving the journal at `journal_root`.
///
/// # Panics
///
/// If `token` is empty.
pub fn router(journal_root: PathBuf, token: String) -> Router {
  assert!(!token.is_empty(), "serve needs a token");
  let shared = Arc::new(Shared {
    control: control::dir(&journal_root),
    journal_root,
    token,
  });
  Router::new()
    .route("/control", get(list))
    .route("/control/{instance}", put(push).delete(remove))
    .route("/manifest", get(manifest))
    .route("/files/{*path}", get(file))
    .with_state(shared)
}

/// Whether the request carries the token. Compared in constant time.
fn authorized(shared: &Shared, headers: &HeaderMap) -> bool {
  let Some(given) = headers
    .get(AUTHORIZATION)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.strip_prefix("Bearer "))
  else {
    return false;
  };
  let expected = shared.token.as_bytes();
  let given = given.as_bytes();
  given.len() == expected.len()
    && given
      .iter()
      .zip(expected)
      .fold(0, |difference, (a, b)| difference | (a ^ b))
      == 0
}

async fn push(
  State(shared): State<Arc<Shared>>,
  Path(instance): Path<String>,
  headers: HeaderMap,
  body: String,
) -> StatusCode {
  if !authorized(&shared, &headers) {
    return StatusCode::UNAUTHORIZED;
  }
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
  if !authorized(&shared, &headers) {
    return StatusCode::UNAUTHORIZED;
  }
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
  if !authorized(&shared, &headers) {
    return Err(StatusCode::UNAUTHORIZED);
  }
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
  if !authorized(&shared, &headers) {
    return Err(StatusCode::UNAUTHORIZED);
  }
  let files = manifest::list(&shared.journal_root)
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  Ok(manifest::encode(&files))
}

/// A live file's or archive's bytes, from the `Range: bytes=<from>-` offset if given.
async fn file(
  State(shared): State<Arc<Shared>>,
  Path(path): Path<String>,
  headers: HeaderMap,
) -> Result<(StatusCode, Vec<u8>), StatusCode> {
  if !authorized(&shared, &headers) {
    return Err(StatusCode::UNAUTHORIZED);
  }
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
  if from.is_some_and(|from| from > size) {
    return Err(StatusCode::RANGE_NOT_SATISFIABLE);
  }
  opened
    .seek(SeekFrom::Start(from.unwrap_or(0)))
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  let mut bytes = Vec::new();
  opened
    .read_to_end(&mut bytes)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
  let status = match from {
    Some(_) => StatusCode::PARTIAL_CONTENT,
    None => StatusCode::OK,
  };
  Ok((status, bytes))
}
