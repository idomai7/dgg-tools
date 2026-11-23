//! Format version 0, pre-release: any minor may break, so readers accept
//! only the minor they know.
//!
//! After the `journal 0.<minor>` header, each line is
//!
//! ```text
//! <receive-time> <instance> <connection> <kind> <data>
//! ```
//!
//! separated by single spaces. `receive-time` (local receive time, unix ms)
//! and `connection` are decimal digits, `instance` is `[A-Za-z0-9._-]+`, and
//! `kind` is `[a-z0-9]+`. `data` is the rest of the line, verbatim; the space
//! before it is omitted when it's empty. Kinds:
//!
//! - `frame`: a dgg text frame exactly as received (`EVENT payload`).
//! - `binary`: a binary frame, base64.
//! - `open`: a connection opened; data is the URL.
//! - `close`: a connection dropped; data is why.
//! - `fetched`: a message fetched from another source to fill a gap; data is
//!   `<source> <timestamp> <nick> <text>`, with `timestamp` dgg's unix ms and
//!   `text` verbatim to the end of the line. (0.1)
//! - `fetchrange`: a source returned everything it has between two times;
//!   data is `<source> <from> <to>`, unix ms. (0.1)
//!
//! `source` and `nick` are [tokens](valid_token). Lines end at LF only; a CR
//! is ordinary data. Data containing an LF can't sit on one line, so it's
//! written with the kind suffixed `64` and the data base64 (`frame64`,
//! `open64`, `close64`, `fetched64`).
//!
//! Minor versions:
//!
//! - `0.0`: `frame`, `binary`, `open` and `close`.
//! - `0.1`: adds `fetched` and `fetchrange`.

pub use super::line::{valid_instance, valid_token};
use {
  super::{DecodeError, Version, digits, line},
  ::std::borrow::Cow,
};

pub const VERSION: Version = Version { major: 0, minor: 1 };

/// One line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
  /// Local receive time, unix ms.
  pub receive_time: i64,
  /// Capture instance that wrote the line.
  pub instance: String,
  /// Connection number within the instance, counting from 1.
  pub connection: u64,
  pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
  /// A dgg text frame exactly as received (`EVENT payload`).
  Frame(String),
  /// A binary frame. dgg doesn't send these today; they're kept so a
  /// protocol change can't silently lose data.
  Binary(Vec<u8>),
  /// Connection opened, with the URL it connected to.
  Open(String),
  /// Connection dropped, with why.
  Close(String),
  /// A message fetched from another source to fill a gap. (0.1)
  Fetched {
    source: String,
    /// dgg's timestamp, unix ms.
    timestamp: i64,
    nick: String,
    text: String,
  },
  /// `source` returned everything it has from `from` to `to`, unix ms.
  /// (0.1)
  FetchRange { source: String, from: i64, to: i64 },
}

impl Record {
  /// Appends this record's line, newline included, to `out`.
  ///
  /// # Panics
  ///
  /// If a `source` or `nick` isn't a [`valid_token`]; it couldn't be read
  /// back.
  pub fn encode(&self, out: &mut Vec<u8>) {
    let tokens = |tokens: &[&str]| {
      for token in tokens {
        assert!(valid_token(token), "not a valid token: {token:?}");
      }
    };
    let (kind, data) = match &self.body {
      Body::Frame(text) => ("frame", Cow::Borrowed(text.as_bytes())),
      Body::Open(text) => ("open", Cow::Borrowed(text.as_bytes())),
      Body::Close(text) => ("close", Cow::Borrowed(text.as_bytes())),
      Body::Binary(bytes) => ("binary", Cow::Borrowed(bytes.as_slice())),
      Body::Fetched {
        source,
        timestamp,
        nick,
        text,
      } => {
        tokens(&[source, nick]);
        let data = format!("{source} {timestamp} {nick} {text}");
        ("fetched", Cow::Owned(data.into_bytes()))
      }
      Body::FetchRange { source, from, to } => {
        tokens(&[source]);
        let data = format!("{source} {from} {to}");
        ("fetchrange", Cow::Owned(data.into_bytes()))
      }
    };
    let binary = matches!(self.body, Body::Binary(_));
    line::encode(
      out,
      self.receive_time,
      &self.instance,
      self.connection,
      kind,
      &data,
      binary,
    );
  }

  /// Decodes one line, without its newline.
  pub fn decode(line: &[u8]) -> Result<Self, DecodeError> {
    let line = line::decode(line)?;
    let data = line.data;
    let body = match line.kind {
      "frame" => Body::Frame(data.into()),
      "open" => Body::Open(data.into()),
      "close" => Body::Close(data.into()),
      "frame64" => Body::Frame(line.text64()?),
      "open64" => Body::Open(line.text64()?),
      "close64" => Body::Close(line.text64()?),
      "binary" => Body::Binary(line.unbase64()?),
      "fetched" => fetched(data)?,
      "fetched64" => fetched(&line.text64()?)?,
      "fetchrange" => {
        let mut parts = data.split(' ');
        let (Some(source), Some(from), Some(to), None) =
          (parts.next(), parts.next(), parts.next(), parts.next())
        else {
          return Err(DecodeError::Bad("fetchrange needs source, from and to"));
        };
        Body::FetchRange {
          source: token(source)?,
          from: number(from)?,
          to: number(to)?,
        }
      }
      kind => return Err(DecodeError::Unknown(kind.into())),
    };
    Ok(Self {
      receive_time: line.receive_time,
      instance: line.instance.into(),
      connection: line.connection,
      body,
    })
  }
}

/// Decodes a `fetched` line's data: `<source> <timestamp> <nick> <text>`.
fn fetched(data: &str) -> Result<Body, DecodeError> {
  let mut parts = data.splitn(4, ' ');
  let (Some(source), Some(timestamp), Some(nick)) =
    (parts.next(), parts.next(), parts.next())
  else {
    return Err(DecodeError::Bad("fetched needs source, timestamp and nick"));
  };
  Ok(Body::Fetched {
    source: token(source)?,
    timestamp: number(timestamp)?,
    nick: token(nick)?,
    text: parts.next().unwrap_or("").into(),
  })
}

fn token(text: &str) -> Result<String, DecodeError> {
  if valid_token(text) {
    Ok(text.into())
  } else {
    Err(DecodeError::Bad("bad token"))
  }
}

fn number(text: &str) -> Result<i64, DecodeError> {
  if !digits(text) {
    return Err(DecodeError::Bad("not decimal"));
  }
  text
    .parse()
    .map_err(|_| DecodeError::Bad("number out of range"))
}
