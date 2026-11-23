//! Kick format version 0, pre-release: any minor may break, so readers
//! accept only the minor they know.
//!
//! After the `journal kick.0.<minor>` header, a line ending at LF, each
//! record is
//!
//! ```text
//! <receive-time> <instance> <connection> <kind> <data><RS><LF>
//! ```
//!
//! with `RS` the record separator, 0x1E, as in dgg's (see
//! [`record::v0`](super::super::v0)). `receive-time` is the local receive
//! time, unix ms, and `connection` the connection number within the
//! instance, counting from 1. `data` is verbatim, LFs included, so a record
//! may span lines. Kinds:
//!
//! - `frame`: a Centrifugo text frame exactly as received: one or more JSON
//!   replies, separated by LFs.
//! - `binary`: a binary frame, base64.
//! - `open`: a connection opened (Centrifugo accepted the connect); data is
//!   the socket's URL.
//! - `close`: a connection dropped or stopped; data is why.
//!
//! An RS only ever ends a record: text data containing one is written with
//! the kind suffixed `64` and the data base64 (`frame64`, `open64`,
//! `close64`). Valid JSON can't hold an RS, so frames almost never are.
//!
//! Minor versions:
//!
//! - `0.0`: `frame`, `binary`, `open` and `close`.

use {
  super::super::{
    DecodeError, Version,
    line::{self, Framing},
  },
  ::std::borrow::Cow,
};

pub const VERSION: Version = Version { major: 0, minor: 0 };

/// One record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
  /// Local receive time, unix ms.
  pub receive_time: i64,
  /// Instance that wrote the line.
  pub instance: String,
  /// Connection number within the instance, counting from 1.
  pub connection: u64,
  pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
  /// A Centrifugo text frame exactly as received.
  Frame(String),
  /// A binary frame. A JSON client shouldn't get these; they're kept so a
  /// protocol change can't silently lose data.
  Binary(Vec<u8>),
  /// Connection opened, with the socket's URL.
  Open(String),
  /// Connection dropped or stopped, with why.
  Close(String),
}

impl Record {
  /// Appends this record, with its RS LF, to `out`.
  pub fn encode(&self, out: &mut Vec<u8>) {
    let (kind, data) = match &self.body {
      Body::Frame(text) => ("frame", Cow::Borrowed(text.as_bytes())),
      Body::Open(text) => ("open", Cow::Borrowed(text.as_bytes())),
      Body::Close(text) => ("close", Cow::Borrowed(text.as_bytes())),
      Body::Binary(bytes) => ("binary", Cow::Borrowed(bytes.as_slice())),
    };
    let binary = matches!(self.body, Body::Binary(_));
    line::encode(
      out,
      Framing::RsLf,
      self.receive_time,
      &self.instance,
      self.connection,
      kind,
      &data,
      binary,
    );
  }

  /// Decodes one record, without its RS LF.
  pub fn decode(line: &[u8]) -> Result<Self, DecodeError> {
    let line = line::decode(Framing::RsLf, line)?;
    let body = match line.kind {
      "frame" => Body::Frame(line.data.into()),
      "open" => Body::Open(line.data.into()),
      "close" => Body::Close(line.data.into()),
      "frame64" => Body::Frame(line.text64()?),
      "open64" => Body::Open(line.text64()?),
      "close64" => Body::Close(line.text64()?),
      "binary" => Body::Binary(line.unbase64()?),
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
