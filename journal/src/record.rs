//! A decoded line from a file of any readable version.

use crate::{v1, version::Version};

/// A record from a file of any readable version, tagged by its major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
  V1(v1::Record),
}

/// Why a complete line couldn't be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
  /// Not a well-formed line, e.g. one torn by a crash.
  Bad(&'static str),
  /// Well-formed, but a kind this version doesn't know (added by a newer
  /// minor, say).
  Unknown(String),
}

impl Record {
  /// Decodes one line, without its newline, from a file at `version`.
  ///
  /// # Panics
  ///
  /// If `version` isn't [`Version::readable`].
  pub fn decode(version: Version, line: &[u8]) -> Result<Self, DecodeError> {
    match version.major {
      1 => v1::Record::decode(line).map(Self::V1),
      major => panic!("no decoder for major version {major}"),
    }
  }
}
