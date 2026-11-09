//! The peer an install pulls its journal from: `peer.env` in the install,
//! read by the supervisor before each scheduled `sync`, so editing it
//! takes effect without reinstalling. Its token never goes on a command
//! line: `sync` gets it in its environment.
//!
//! The file holds `KEY=VALUE` lines ([`URL`], [`TOKEN`]); blank lines and
//! `#` comments are skipped, and a value may be quoted.

use ::std::{fs, io, path::Path};

/// The peer's `serve`, e.g. `http://old-host:8788`.
pub const URL: &str = "LOGGER_PEER_URL";
/// Its bearer token, as `dgg-logger sync` reads it from the environment.
pub const TOKEN: &str = "LOGGER_PEER_TOKEN";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
  pub url: String,
  pub token: String,
}

/// The peer in `path`; `None` if there's no file.
pub fn read(path: &Path) -> io::Result<Option<Peer>> {
  let text = match fs::read_to_string(path) {
    Ok(text) => text,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(e),
  };
  let (mut url, mut token) = (None, None);
  for line in text.lines().map(str::trim) {
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let Some((key, value)) = line.split_once('=') else {
      return Err(invalid(path, &format!("not KEY=VALUE: {line:?}")));
    };
    let value = unquote(value.trim()).to_owned();
    match key.trim() {
      URL => url = Some(value),
      TOKEN => token = Some(value),
      _ => {}
    }
  }
  match (url, token) {
    (Some(url), Some(token)) if !url.is_empty() && !token.is_empty() => {
      Ok(Some(Peer { url, token }))
    }
    _ => Err(invalid(path, &format!("needs {URL} and {TOKEN}"))),
  }
}

fn unquote(value: &str) -> &str {
  for quote in ['"', '\''] {
    if let Some(inner) = value
      .strip_prefix(quote)
      .and_then(|value| value.strip_suffix(quote))
    {
      return inner;
    }
  }
  value
}

fn invalid(path: &Path, what: &str) -> io::Error {
  io::Error::new(
    io::ErrorKind::InvalidData,
    format!("{}: {what}", path.display()),
  )
}
