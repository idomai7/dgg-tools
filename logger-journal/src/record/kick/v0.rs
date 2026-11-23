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
    fields::{self, Framing},
  },
  ::std::borrow::Cow,
};

pub const VERSION: Version = Version { major: 0, minor: 0 };

/// One record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
  /// Local receive time, unix ms.
  pub receive_time: i64,
  /// Instance that wrote the record.
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
    fields::encode(
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
  pub fn decode(record: &[u8]) -> Result<Self, DecodeError> {
    let fields = fields::decode(Framing::RsLf, record)?;
    let body = match fields.kind {
      "frame" => Body::Frame(fields.data.into()),
      "open" => Body::Open(fields.data.into()),
      "close" => Body::Close(fields.data.into()),
      "frame64" => Body::Frame(fields.text64()?),
      "open64" => Body::Open(fields.text64()?),
      "close64" => Body::Close(fields.text64()?),
      "binary" => Body::Binary(fields.unbase64()?),
      kind => return Err(DecodeError::Unknown(kind.into())),
    };
    Ok(Self {
      receive_time: fields.receive_time,
      instance: fields.instance.into(),
      connection: fields.connection,
      body,
    })
  }
}
