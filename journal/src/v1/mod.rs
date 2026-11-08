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
//! Text containing a CR or LF can't sit on one line, so it's written with the
//! kind suffixed `64` and the data base64 (`frame64`, `open64`, `close64`).
//!
//! Minor versions:
//!
//! - `1.0`: the above.

mod decode;
mod encode;

use crate::version::Version;

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

pub fn valid_inst(inst: &str) -> bool {
  !inst.is_empty()
    && inst
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn valid_kind(kind: &str) -> bool {
  !kind.is_empty()
    && kind
      .bytes()
      .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}
