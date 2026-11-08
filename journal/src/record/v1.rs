//! Format version 1.
//!
//! After the `journal 1.<minor>` header, each line is
//!
//! ```text
//! <recv> <inst> <conn> <kind> <data>
//! ```
//!
//! separated by single spaces. `recv` (local receive time, unix ms) and
//! `conn` are decimal digits, `inst` is `[A-Za-z0-9._-]+`, and `kind` is
//! `[a-z0-9]+`. `data` is the rest of the line, verbatim; the space before it
//! is omitted when it's empty. Kinds:
//!
//! - `frame`: a dgg text frame exactly as received (`EVENT payload`).
//! - `binary`: a binary frame, base64.
//! - `open`: a connection opened; data is the URL.
//! - `close`: a connection dropped; data is why.
//!
//! Lines end at LF only; a CR is ordinary data. Text containing an LF can't
//! sit on one line, so it's written with the kind suffixed `64` and the data
//! base64 (`frame64`, `open64`, `close64`).
//!
//! Minor versions:
//!
//! - `1.0`: the above.

use {
  super::{DecodeError, Version, digits},
  ::base64::{Engine, engine::general_purpose::STANDARD as BASE64},
};

pub const VERSION: Version = Version { major: 1, minor: 0 };

/// One line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
  /// Local receive time, unix ms.
  pub recv: i64,
  /// Capture instance that wrote the line.
  pub inst: String,
  /// Connection number within the instance, counting from 1.
  pub conn: u64,
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
}

/// Whether `inst` is a valid instance id: one or more of `[A-Za-z0-9._-]`,
/// so it can't contain the field separator and is safe as a file name.
pub fn valid_inst(inst: &str) -> bool {
  !inst.is_empty()
    && inst
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Whether `kind` is well-formed: one or more of `[a-z0-9]`. A well-formed
/// kind this version doesn't recognise is [`DecodeError::Unknown`], not
/// [`DecodeError::Bad`].
fn valid_kind(kind: &str) -> bool {
  !kind.is_empty()
    && kind
      .bytes()
      .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

impl Record {
  /// Appends this record's line, newline included, to `out`.
  pub fn encode(&self, out: &mut Vec<u8>) {
    let (kind, data) = match &self.body {
      Body::Frame(t) => ("frame", t.as_bytes()),
      Body::Open(t) => ("open", t.as_bytes()),
      Body::Close(t) => ("close", t.as_bytes()),
      Body::Binary(b) => ("binary", b.as_slice()),
    };
    let binary = matches!(self.body, Body::Binary(_));
    let text64 = !binary && data.contains(&b'\n');
    let suffix = if text64 { "64" } else { "" };
    let header =
      format!("{} {} {} {kind}{suffix}", self.recv, self.inst, self.conn);
    out.extend_from_slice(header.as_bytes());
    if !data.is_empty() {
      out.push(b' ');
      if binary || text64 {
        out.extend_from_slice(BASE64.encode(data).as_bytes());
      } else {
        out.extend_from_slice(data);
      }
    }
    out.push(b'\n');
  }

  /// Decodes one line, without its newline.
  pub fn decode(line: &[u8]) -> Result<Self, DecodeError> {
    use DecodeError::Bad;
    let line = ::std::str::from_utf8(line).map_err(|_| Bad("not utf-8"))?;
    let mut parts = line.splitn(5, ' ');
    let mut field = || parts.next().ok_or(Bad("too few fields"));
    let (recv, inst, conn, kind) = (field()?, field()?, field()?, field()?);
    let data = parts.next().unwrap_or("");
    if !digits(recv) || !digits(conn) {
      return Err(Bad("recv/conn not decimal"));
    }
    if !valid_inst(inst) {
      return Err(Bad("bad inst"));
    }
    if !valid_kind(kind) {
      return Err(Bad("bad kind"));
    }
    let recv = recv.parse().map_err(|_| Bad("recv out of range"))?;
    let conn = conn.parse().map_err(|_| Bad("conn out of range"))?;
    let unbase64 = || BASE64.decode(data).map_err(|_| Bad("bad base64"));
    let text64 =
      || String::from_utf8(unbase64()?).map_err(|_| Bad("base64 not utf-8"));
    let body = match kind {
      "frame" => Body::Frame(data.into()),
      "open" => Body::Open(data.into()),
      "close" => Body::Close(data.into()),
      "frame64" => Body::Frame(text64()?),
      "open64" => Body::Open(text64()?),
      "close64" => Body::Close(text64()?),
      "binary" => Body::Binary(unbase64()?),
      _ => return Err(DecodeError::Unknown(kind.into())),
    };
    Ok(Self {
      recv,
      inst: inst.into(),
      conn,
      body,
    })
  }
}
