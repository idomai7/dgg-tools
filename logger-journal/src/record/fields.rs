//! The record shape every format so far shares:
//!
//! ```text
//! <receive-time> <instance> <connection> <kind> <data>
//! ```
//!
//! separated by single spaces. `receive-time` (local receive time, unix ms)
//! and `connection` are decimal digits, `instance` is `[A-Za-z0-9._-]+`, and
//! `kind` is `[a-z0-9]+`. `data` is the rest of the record, verbatim; the
//! space before it is omitted when it's empty. How a record ends is its
//! file's [`Framing`]. Text data holding the byte that framing reserves is
//! written with the kind suffixed `64` and the data base64; binary data is
//! always base64.

use {
  super::{DecodeError, digits},
  ::base64::{Engine, engine::general_purpose::STANDARD as BASE64},
};

/// The record separator, ASCII 0x1E.
pub const RS: u8 = 0x1e;

/// How a file's records end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
  /// Each record is one line, ending at LF; a CR is ordinary data. Text
  /// data holding an LF goes base64. dgg's, up to `journal 0.1`.
  Lf,
  /// Each record ends at RS LF, and its data holds LFs (and CRs) verbatim,
  /// so a record may span lines. Text data holding an RS goes base64, so an
  /// RS in a file only ever ends a record.
  RsLf,
}

impl Framing {
  /// What ends each record.
  pub fn end(self) -> &'static [u8] {
    match self {
      Self::Lf => b"\n",
      Self::RsLf => &[RS, b'\n'],
    }
  }

  /// The byte text data can't hold verbatim.
  fn reserved(self) -> u8 {
    match self {
      Self::Lf => b'\n',
      Self::RsLf => RS,
    }
  }

  /// Splits `bytes`, the records after a file's header, into its complete
  /// records, without their ends, and what follows the last: empty, or a
  /// torn record.
  pub fn split(self, bytes: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let end = self.end();
    let mut records = Vec::new();
    let mut rest = bytes;
    while let Some(at) = rest.windows(end.len()).position(|w| w == end) {
      records.push(&rest[..at]);
      rest = &rest[at + end.len()..];
    }
    (records, rest)
  }
}

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

/// A record's fields, its data still encoded.
pub(crate) struct Fields<'a> {
  pub receive_time: i64,
  pub instance: &'a str,
  pub connection: u64,
  pub kind: &'a str,
  pub data: &'a str,
}

/// Appends a record, with its end, to `out`. `binary` data is always
/// base64; text data only when it holds the byte `framing` reserves, with
/// `kind` suffixed `64`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode(
  out: &mut Vec<u8>,
  framing: Framing,
  receive_time: i64,
  instance: &str,
  connection: u64,
  kind: &str,
  data: &[u8],
  binary: bool,
) {
  let text64 = !binary && data.contains(&framing.reserved());
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
  out.extend_from_slice(framing.end());
}

/// Splits one record, without its end, into its fields.
pub(crate) fn decode(
  framing: Framing,
  record: &[u8],
) -> Result<Fields<'_>, DecodeError> {
  use DecodeError::Bad;
  if record.contains(&framing.reserved()) {
    return Err(Bad("the record's end inside it"));
  }
  let record = ::std::str::from_utf8(record).map_err(|_| Bad("not utf-8"))?;
  let mut parts = record.splitn(5, ' ');
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
  Ok(Fields {
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

impl Fields<'_> {
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
