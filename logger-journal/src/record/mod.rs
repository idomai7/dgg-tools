//! A decoded record from a file of any readable variant and version.
//!
//! The header line every journal file starts with (see [`Header`]):
//! `journal dgg.<major>.<minor>` for dgg, `journal kick.<major>.<minor>` for
//! Kick, each optionally followed by ` ;` and informational fields. dgg's
//! files up to 0.1 named no variant, `journal 0.1`; they're no longer
//! read.

mod fields;
pub mod kick;
pub mod v0;

pub use fields::{END, RS, split};

/// Format name at the start of every file's header line.
pub const MAGIC: &str = "journal";

/// A format with a [`Header`] line: the journal, and the files that
/// describe journals and their capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
  /// The name its header lines start with.
  pub magic: &'static str,
}

/// The journal: `journal dgg.0.3` and `journal kick.0.0`.
pub const JOURNAL: Format = Format { magic: MAGIC };
/// The versions of dgg's format this crate has a decoder for.
pub const READABLE: &[Version] = &[v0::VERSION];

/// Which platform's format a file is in. Each is versioned on its own, so one
/// platform's changes never break the other's files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
  /// dgg's, the first, `journal dgg.<major>.<minor>`, its majors under
  /// [`v0`].
  Dgg,
  /// Kick's, `journal kick.<major>.<minor>`, its majors under [`kick`].
  Kick,
}

impl Variant {
  /// The versions this crate has a decoder for: within major 0, each minor;
  /// from major 1 on, the newest of each major.
  pub fn readable(self) -> &'static [Version] {
    match self {
      Self::Dgg => READABLE,
      Self::Kick => kick::READABLE,
    }
  }

  /// Its name in headers.
  fn name(self) -> &'static str {
    match self {
      Self::Dgg => "dgg",
      Self::Kick => "kick",
    }
  }
}

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
  /// Whether this crate can read dgg files written at this version: within
  /// major 0, only the exact minors it knows; from major 1 on, any minor of
  /// a major it knows.
  pub fn readable(self) -> bool {
    self.readable_in(Variant::Dgg)
  }

  /// [`Version::readable`], for files of `variant`.
  pub fn readable_in(self, variant: Variant) -> bool {
    variant.readable().iter().any(|known| {
      known.major == self.major && (self.major > 0 || known.minor == self.minor)
    })
  }
}

/// A file's header line: `journal <variant>.<version>`, then optionally ` ;`
/// and informational `key=value` fields, separated by single spaces. It ends
/// at RS LF ([`END`]), as records do, so a reader finds it at the first LF.
/// For example:
///
/// ```text
/// journal kick.0.0 ; endpoint=wss://example.com/connection channel=destiny
/// ```
///
/// Only the variant and version are required, and a new required field
/// changes the format. The informational fields are redundant information
/// and notes, safe to ignore: readers skip keys they don't know, so a new key
/// never changes the format. Keys are `[a-z0-9_]+`, and values are
/// [tokens](v0::valid_token).
///
/// The manifests describing journals have headers of the same form, with
/// their own [`Format`]s: see [`Header::parse_as`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
  pub variant: Variant,
  pub version: Version,
  pub info: Vec<(String, String)>,
}

impl Header {
  pub fn new(variant: Variant, version: Version) -> Self {
    Self {
      variant,
      version,
      info: Vec::new(),
    }
  }

  /// With the informational field `key=value` added.
  ///
  /// # Panics
  ///
  /// If `key` isn't `[a-z0-9_]+` or `value` isn't a
  /// [token](v0::valid_token); they couldn't be read back.
  pub fn with(mut self, key: &str, value: &str) -> Self {
    assert!(valid_key(key), "not a valid key: {key:?}");
    assert!(fields::valid_token(value), "not a valid value: {value:?}");
    self.info.push((key.into(), value.into()));
    self
  }

  /// The first value of the informational field `key`.
  pub fn get(&self, key: &str) -> Option<&str> {
    let (_, value) = self.info.iter().find(|(k, _)| k == key)?;
    Some(value)
  }

  /// Whether this crate can read the file (see [`Version::readable`]).
  pub fn readable(&self) -> bool {
    self.version.readable_in(self.variant)
  }

  /// The journal's header line, without its end.
  pub fn line(&self) -> String {
    self.line_as(JOURNAL)
  }

  /// The journal's header line, with its end.
  pub fn text(&self) -> String {
    self.text_as(JOURNAL)
  }

  /// The header line in `format`, with its end, RS LF.
  pub fn text_as(&self, format: Format) -> String {
    self.line_as(format) + "\u{1e}\n"
  }

  /// The header line in `format`, without its end.
  pub fn line_as(&self, format: Format) -> String {
    let Version { major, minor } = self.version;
    let magic = format.magic;
    let variant = self.variant.name();
    let mut line = format!("{magic} {variant}.{major}.{minor}");
    if !self.info.is_empty() {
      line.push_str(" ;");
      for (key, value) in &self.info {
        line.push_str(&format!(" {key}={value}"));
      }
    }
    line
  }

  /// Parses a journal's header line, up to its LF: with its RS.
  pub fn parse(line: &[u8]) -> Option<Self> {
    Self::parse_as(JOURNAL, line)
  }

  /// Parses a header line in `format`, up to its LF: with its RS. Whether
  /// its version is one the reader knows is left to it.
  pub fn parse_as(format: Format, line: &[u8]) -> Option<Self> {
    let line = line.strip_suffix(&[RS])?;
    let line = ::std::str::from_utf8(line).ok()?;
    let rest = line.strip_prefix(format.magic)?.strip_prefix(' ')?;
    let (required, info) = match rest.split_once(" ;") {
      Some((required, info)) => (required, Some(info.strip_prefix(' ')?)),
      None => (rest, None),
    };
    let parts: Vec<&str> = required.split('.').collect();
    let (variant, major, minor) = match parts[..] {
      ["dgg", major, minor] => (Variant::Dgg, major, minor),
      ["kick", major, minor] => (Variant::Kick, major, minor),
      _ => return None,
    };
    if !digits(major) || !digits(minor) {
      return None;
    }
    let version = Version {
      major: major.parse().ok()?,
      minor: minor.parse().ok()?,
    };
    let mut header = Self::new(variant, version);
    for field in info.into_iter().flat_map(|info| info.split(' ')) {
      let (key, value) = field.split_once('=')?;
      if !valid_key(key) || !fields::valid_token(value) {
        return None;
      }
      header.info.push((key.into(), value.into()));
    }
    Some(header)
  }
}

fn valid_key(key: &str) -> bool {
  !key.is_empty()
    && key
      .bytes()
      .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A record from a file of any readable variant and version, tagged by both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
  V0(v0::Record),
  Kick0(kick::v0::Record),
}

/// Why a complete record couldn't be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
  /// Not a well-formed record, e.g. one torn by a crash.
  Bad(&'static str),
  /// Well-formed, but a kind this version doesn't know (added by a newer
  /// minor, say).
  Unknown(String),
  /// No decoder for this major version.
  NoDecoder(u32),
}

impl Record {
  /// Decodes one record, without its end, from a file with `header`.
  pub fn decode(header: &Header, record: &[u8]) -> Result<Self, DecodeError> {
    match (header.variant, header.version.major) {
      (Variant::Dgg, 0) => v0::Record::decode(record).map(Self::V0),
      (Variant::Kick, 0) => kick::v0::Record::decode(record).map(Self::Kick0),
      (_, major) => Err(DecodeError::NoDecoder(major)),
    }
  }
}

pub(crate) fn digits(text: &str) -> bool {
  !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}
