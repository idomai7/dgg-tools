//! Archived days and the monthly manifests describing them.
//!
//! An archive is an ordinary journal file (header and records), zstd-
//! compressed, holding every record of one UTC day. Each month's directory
//! (`archive/YYYY/MM`) has a `manifest` listing its archives, so readers
//! learn what's archived from about a dozen small files a year instead of
//! opening every archive:
//!
//! ```text
//! journal-archive dgg.0.2<RS><LF>
//! <YYYY-MM-DD> <compressed bytes> <bytes> <records> <sha-256><RS><LF>
//! ```
//!
//! The header and each entry end at RS LF, as the journal's records do.
//! `bytes` and `records` are of the decompressed journal, `records` counted
//! by its framing, after its header; the SHA-256 is of the compressed file,
//! lowercase hex.
//!
//! The header is versioned like the journal's (see
//! [`Header`](crate::Header)): each variant on its own, `journal-archive
//! dgg.0.2` and `journal-archive kick.0.0`, optionally with informational
//! fields. While the major is 0 (pre-release) any minor may break, so readers
//! accept only the versions they know. From 1.0 on, readers will ignore
//! lines they don't recognise, so a minor bump may add kinds of entry;
//! anything else will be a major bump.
//!
//! Versions of dgg's:
//!
//! - `0.0`: `journal-archive 0.0`, naming no variant, its third count called
//!   `lines`: an archive's records were its lines then. Still read, and
//!   written again as 0.1 when its month's manifest is next recorded.
//! - `0.1`: names the variant; counts records; entries end at RS LF, not
//!   LF, but the header at LF. Still read.
//! - `0.2`: the header ends at RS LF too.

use {
  crate::{
    layout::{archive_dir, archive_file, archive_month_dir},
    record::{Format, Header, Variant, Version},
  },
  ::common::{atomic_file, fs::MissingOk},
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// The manifest's format.
pub const FORMAT: Format = Format {
  magic: "journal-archive",
  last_unnamed: Version { major: 0, minor: 0 },
  rs_header: &[
    (Variant::Dgg, Version { major: 0, minor: 2 }),
    (Variant::Kick, Version { major: 0, minor: 0 }),
  ],
};

/// The version of `variant`'s manifest this crate writes.
pub fn version(variant: Variant) -> Version {
  match variant {
    Variant::Dgg => Version { major: 0, minor: 2 },
    Variant::Kick => Version { major: 0, minor: 0 },
  }
}

/// Whether this crate reads `variant`'s manifest at `version`.
fn readable(variant: Variant, version: Version) -> bool {
  version == self::version(variant)
    || (variant == Variant::Dgg && version.major == 0 && version.minor < 2)
}

/// The manifest's file name in each month directory.
pub const MANIFEST: &str = "manifest";

/// One archived day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
  /// `YYYY-MM-DD`.
  pub day: String,
  /// Size of the compressed file.
  pub compressed: u64,
  /// Size of the decompressed journal.
  pub bytes: u64,
  /// Records in the decompressed journal, header excluded.
  pub records: u64,
  /// SHA-256 of the compressed file, lowercase hex.
  pub sha256: String,
}

impl Archive {
  /// Where this archive lives under journal `root`.
  pub fn path(&self, root: &Path) -> PathBuf {
    archive_file(root, &self.day)
  }
}

/// Whether `day` is `YYYY-MM-DD`.
pub fn valid_day(day: &str) -> bool {
  day.len() == 10
    && day.bytes().enumerate().all(|(i, b)| match i {
      4 | 7 => b == b'-',
      _ => b.is_ascii_digit(),
    })
}

fn digits(text: &str) -> bool {
  !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// A manifest of `variant`'s archives, days in order.
pub fn encode(variant: Variant, archives: &[Archive]) -> String {
  let mut archives = archives.to_vec();
  archives.sort_by(|a, b| a.day.cmp(&b.day));
  let mut text = Header::new(variant, version(variant)).text_as(FORMAT);
  for archive in archives {
    text += &format!(
      "{} {} {} {} {}\u{1e}\n",
      archive.day,
      archive.compressed,
      archive.bytes,
      archive.records,
      archive.sha256
    );
  }
  text
}

/// Parses a manifest: its variant and archives, or `None` if it isn't one
/// this crate can read.
pub fn decode(text: &str) -> Option<(Variant, Vec<Archive>)> {
  let (first, body) = text.split_once('\n')?;
  let header = Header::parse_as(FORMAT, first.as_bytes())?;
  if !readable(header.variant, header.version) {
    return None;
  }
  let (entries, tail) = header.framing_as(FORMAT).split(body.as_bytes());
  if !tail.is_empty() {
    return None;
  }
  let mut archives = Vec::new();
  for entry in entries {
    let line = ::std::str::from_utf8(entry).ok()?;
    let fields: Vec<_> = line.split(' ').collect();
    let [day, compressed, bytes, count, sha256] = fields[..] else {
      continue;
    };
    if !valid_day(day) {
      continue;
    }
    let hex = sha256.len() == 64
      && sha256
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !(digits(compressed) && digits(bytes) && digits(count) && hex) {
      return None;
    }
    archives.push(Archive {
      day: day.into(),
      compressed: compressed.parse().ok()?,
      bytes: bytes.parse().ok()?,
      records: count.parse().ok()?,
      sha256: sha256.into(),
    });
  }
  Some((header.variant, archives))
}

/// The manifest of the month containing `day`.
pub fn manifest_path(root: &Path, day: &str) -> PathBuf {
  archive_month_dir(root, day).join(MANIFEST)
}

/// The archives listed for the month containing `day`; none if the month
/// has no manifest.
pub fn read_month(root: &Path, day: &str) -> io::Result<Vec<Archive>> {
  Ok(read_manifest(&manifest_path(root, day))?.map_or_else(Vec::new, |m| m.1))
}

/// A manifest's variant and archives; `None` if there's none at `path`.
fn read_manifest(path: &Path) -> io::Result<Option<(Variant, Vec<Archive>)>> {
  let Some(text) = fs::read_to_string(path).missing_ok()? else {
    return Ok(None);
  };
  let invalid = |why: &str| {
    let why = format!("{}: {why}", path.display());
    io::Error::new(io::ErrorKind::InvalidData, why)
  };
  decode(&text)
    .map(Some)
    .ok_or_else(|| invalid("not a readable archive manifest"))
}

/// Records `archive`, of `variant`'s journal, in its month's manifest,
/// replacing any entry for the same day. The manifest is written
/// atomically, in this crate's version. Fails if it's another variant's.
pub fn record(
  root: &Path,
  variant: Variant,
  archive: &Archive,
) -> io::Result<()> {
  let path = manifest_path(root, &archive.day);
  let mut archives = match read_manifest(&path)? {
    Some((found, _)) if found != variant => {
      let why = format!("{}: another variant's manifest", path.display());
      return Err(io::Error::new(io::ErrorKind::InvalidData, why));
    }
    Some((_, archives)) => archives,
    None => Vec::new(),
  };
  archives.retain(|other| other.day != archive.day);
  archives.push(archive.clone());
  atomic_file::replace(&path, encode(variant, &archives))
}

/// Every archive under journal `root`, oldest first, read from the monthly
/// manifests.
pub fn all(root: &Path) -> io::Result<Vec<Archive>> {
  let mut archives = Vec::new();
  for year in sorted_dirs(&archive_dir(root))? {
    for month in sorted_dirs(&year)? {
      if let Some((_, listed)) = read_manifest(&month.join(MANIFEST))? {
        archives.extend(listed);
      }
    }
  }
  archives.sort_by(|a, b| a.day.cmp(&b.day));
  Ok(archives)
}

fn sorted_dirs(path: &Path) -> io::Result<Vec<PathBuf>> {
  let Some(entries) = fs::read_dir(path).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut dirs = Vec::new();
  for entry in entries {
    let entry = entry?;
    if entry.file_type()?.is_dir() {
      dirs.push(entry.path());
    }
  }
  dirs.sort();
  Ok(dirs)
}
