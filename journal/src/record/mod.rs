//! A decoded line from a file of any readable version.
//!
//! The header line every journal file starts with: `journal <major>.<minor>`.

pub mod v1;

/// Format name at the start of every file's header line.
pub const MAGIC: &str = "journal";
/// Majors this crate has a decoder for.
pub const READABLE_MAJORS: &[u32] = &[v1::VERSION.major];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
  /// Bumped for changes old readers can't handle.
  pub major: u32,
  /// Bumped for changes old readers tolerate (new kinds).
  pub minor: u32,
}

impl Version {
  /// Whether this crate can read files written at this version.
  pub fn readable(self) -> bool {
    READABLE_MAJORS.contains(&self.major)
  }

  /// The header line, without its newline.
  pub fn header(self) -> String {
    format!("{MAGIC} {}.{}", self.major, self.minor)
  }

  /// Parses a header line, without its newline.
  pub fn parse_header(line: &[u8]) -> Option<Self> {
    let line = ::std::str::from_utf8(line).ok()?;
    let version = line.strip_prefix(MAGIC)?.strip_prefix(' ')?;
    let (major, minor) = version.split_once('.')?;
    if !digits(major) || !digits(minor) {
      return None;
    }
    Some(Self {
      major: major.parse().ok()?,
      minor: minor.parse().ok()?,
    })
  }
}

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
  /// No decoder for this major version.
  NoDecoder(u32),
}

impl Record {
  /// Decodes one line, without its newline, from a file at `version`.
  pub fn decode(version: Version, line: &[u8]) -> Result<Self, DecodeError> {
    match version.major {
      1 => v1::Record::decode(line).map(Self::V1),
      major => Err(DecodeError::NoDecoder(major)),
    }
  }
}

pub(crate) fn digits(text: &str) -> bool {
  !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}
