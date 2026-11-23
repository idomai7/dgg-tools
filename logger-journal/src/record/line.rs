//! The line shape every format so far shares:
//!
//! ```text
//! <receive-time> <instance> <connection> <kind> <data>
//! ```
//!
//! separated by single spaces. `receive-time` (local receive time, unix ms)
//! and `connection` are decimal digits, `instance` is `[A-Za-z0-9._-]+`, and
//! `kind` is `[a-z0-9]+`. `data` is the rest of the line, verbatim; the space
//! before it is omitted when it's empty. Lines end at LF only; a CR is
//! ordinary data. Text data containing an LF can't sit on one line, so it's
//! written with the kind suffixed `64` and the data base64; binary data is
//! always base64.

use {
  super::{DecodeError, digits},
  ::base64::{Engine, engine::general_purpose::STANDARD as BASE64},
};

/// Whether `token` can be a space-separated field: non-empty, with no space
/// or LF.
pub fn valid_token(token: &str) -> bool {
  !token.is_empty() && !token.contains([' ', '\n'])
}

/// Whether `instance` is a valid instance id: one or more of `[A-Za-z0-9._-]`,
/// so it can't contain the field separator and is safe as a file name.
pub fn valid_instance(instance: &str) -> bool {
  !instance.is_empty()
    && instance
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Whether `kind` is well-formed: one or more of `[a-z0-9]`. A well-formed
/// kind a version doesn't recognise is [`DecodeError::Unknown`], not
/// [`DecodeError::Bad`].
pub(crate) fn valid_kind(kind: &str) -> bool {
  !kind.is_empty()
    && kind
      .bytes()
      .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A line's fields, its data still encoded.
pub(crate) struct Line<'a> {
  pub receive_time: i64,
  pub instance: &'a str,
  pub connection: u64,
  pub kind: &'a str,
  pub data: &'a str,
}

/// Appends a line, newline included, to `out`. `binary` data is always
/// base64; text data only when it holds an LF, with `kind` suffixed `64`.
pub(crate) fn encode(
  out: &mut Vec<u8>,
  receive_time: i64,
  instance: &str,
  connection: u64,
  kind: &str,
  data: &[u8],
  binary: bool,
) {
  let text64 = !binary && data.contains(&b'\n');
  let suffix = if text64 { "64" } else { "" };
  let header = format!("{receive_time} {instance} {connection} {kind}{suffix}");
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

/// Splits one line, without its newline, into its fields.
pub(crate) fn decode(line: &[u8]) -> Result<Line<'_>, DecodeError> {
  use DecodeError::Bad;
  let line = ::std::str::from_utf8(line).map_err(|_| Bad("not utf-8"))?;
  let mut parts = line.splitn(5, ' ');
  let mut field = || parts.next().ok_or(Bad("too few fields"));
  let (receive_time, instance, connection, kind) =
    (field()?, field()?, field()?, field()?);
  let data = parts.next().unwrap_or("");
  if !digits(receive_time) || !digits(connection) {
    return Err(Bad("receive_time/connection not decimal"));
  }
  if !valid_instance(instance) {
    return Err(Bad("bad instance"));
  }
  if !valid_kind(kind) {
    return Err(Bad("bad kind"));
  }
  Ok(Line {
    receive_time: receive_time
      .parse()
      .map_err(|_| Bad("receive_time out of range"))?,
    instance,
    connection: connection
      .parse()
      .map_err(|_| Bad("connection out of range"))?,
    kind,
    data,
  })
}

impl Line<'_> {
  /// The data, base64-decoded.
  pub fn unbase64(&self) -> Result<Vec<u8>, DecodeError> {
    BASE64
      .decode(self.data)
      .map_err(|_| DecodeError::Bad("bad base64"))
  }

  /// The data, base64-decoded text.
  pub fn text64(&self) -> Result<String, DecodeError> {
    String::from_utf8(self.unbase64()?)
      .map_err(|_| DecodeError::Bad("base64 not utf-8"))
  }
}
