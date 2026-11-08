//! The header line every journal file starts with: `journal <major>.<minor>`.

use crate::{digits, v1};

/// Format name at the start of every file's header line.
pub const MAGIC: &str = "journal";

/// The version this crate writes.
pub const CURRENT: Version = v1::VERSION;

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
