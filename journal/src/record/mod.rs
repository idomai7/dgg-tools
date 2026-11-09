//! A decoded line from a file of any readable version.
//!
//! The header line every journal file starts with: `journal <major>.<minor>`.

pub mod v0;

/// Format name at the start of every file's header line.
pub const MAGIC: &str = "journal";
/// The newest version of each major this crate has a decoder for.
pub const READABLE: &[Version] = &[v0::VERSION];

/// A format version, like cargo's semver: major 0 is pre-release, where
/// any minor may break; from major 1 on, only a major bump does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
  /// Bumped for changes old readers can't handle.
  pub major: u32,
  /// Within major 0, bumped for any change. From major 1 on, bumped only
  /// for changes old readers tolerate (new kinds).
  pub minor: u32,
}

impl Version {
  /// Whether this crate can read files written at this version: within
  /// major 0, only the exact minor it knows; from major 1 on, any minor of
  /// a major it knows.
  pub fn readable(self) -> bool {
    READABLE.iter().any(|known| {
      known.major == self.major && (self.major > 0 || known.minor == self.minor)
    })
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
  V0(v0::Record),
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
      0 => v0::Record::decode(line).map(Self::V0),
      major => Err(DecodeError::NoDecoder(major)),
    }
  }
}

pub(crate) fn digits(text: &str) -> bool {
  !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}
