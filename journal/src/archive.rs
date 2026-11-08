//! Archived days and the monthly manifests describing them.
//!
//! An archive is an ordinary journal file (header and lines), zstd-
//! compressed, holding every line of one UTC day. Each month's directory
//! (`archive/YYYY/MM`) has a `manifest` listing its archives, so readers
//! learn what's archived from about a dozen small files a year instead of
//! opening every archive:
//!
//! ```text
//! journal-archive 1.0
//! <YYYY-MM-DD> <compressed bytes> <bytes> <lines> <sha-256 of the archive>
//! ```
//!
//! `bytes` and `lines` are of the decompressed journal; the SHA-256 is of
//! the compressed file, lowercase hex. Readers ignore lines they don't
//! recognise, so a minor bump may add kinds of entry; anything else is a
//! major bump.

use {
  crate::layout::{archive_dir, archive_file, archive_month_dir},
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// Format name and major version on a manifest's first line.
pub const HEADER_PREFIX: &str = "journal-archive 1.";

/// The full header line this crate writes.
pub const HEADER: &str = "journal-archive 1.0";

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
  /// Lines in the decompressed journal, header excluded.
  pub lines: u64,
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

/// A manifest's contents, days in order.
pub fn encode(archives: &[Archive]) -> String {
  let mut archives = archives.to_vec();
  archives.sort_by(|a, b| a.day.cmp(&b.day));
  let mut text = format!("{HEADER}\n");
  for archive in archives {
    text += &format!(
      "{} {} {} {} {}\n",
      archive.day,
      archive.compressed,
      archive.bytes,
      archive.lines,
      archive.sha256
    );
  }
  text
}

/// Parses a manifest, or `None` if it isn't one this crate can read.
pub fn decode(text: &str) -> Option<Vec<Archive>> {
  let mut lines = text.strip_suffix('\n')?.split('\n');
  if !digits(lines.next()?.strip_prefix(HEADER_PREFIX)?) {
    return None;
  }
  let mut archives = Vec::new();
  for line in lines {
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
      lines: count.parse().ok()?,
      sha256: sha256.into(),
    });
  }
  Some(archives)
}

/// The manifest of the month containing `day`.
pub fn manifest_path(root: &Path, day: &str) -> PathBuf {
  archive_month_dir(root, day).join(MANIFEST)
}

/// The archives listed for the month containing `day`; none if the month
/// has no manifest.
pub fn read_month(root: &Path, day: &str) -> io::Result<Vec<Archive>> {
  read_manifest(&manifest_path(root, day))
}

fn read_manifest(path: &Path) -> io::Result<Vec<Archive>> {
  match fs::read_to_string(path) {
    Ok(text) => decode(&text).ok_or_else(|| {
      io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: not a readable archive manifest", path.display()),
      )
    }),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
    Err(e) => Err(e),
  }
}

/// Records `archive` in its month's manifest, replacing any entry for the
/// same day. The manifest is written atomically.
pub fn record(root: &Path, archive: &Archive) -> io::Result<()> {
  let path = manifest_path(root, &archive.day);
  let mut archives = read_manifest(&path)?;
  archives.retain(|other| other.day != archive.day);
  archives.push(archive.clone());
  fs::create_dir_all(path.parent().unwrap())?;
  let temporary = path.with_extension("tmp");
  fs::write(&temporary, encode(&archives))?;
  fs::rename(temporary, path)
}

/// Every archive under journal `root`, oldest first, read from the monthly
/// manifests.
pub fn all(root: &Path) -> io::Result<Vec<Archive>> {
  let mut archives = Vec::new();
  for year in sorted_dirs(&archive_dir(root))? {
    for month in sorted_dirs(&year)? {
      archives.extend(read_manifest(&month.join(MANIFEST))?);
    }
  }
  archives.sort_by(|a, b| a.day.cmp(&b.day));
  Ok(archives)
}

fn sorted_dirs(path: &Path) -> io::Result<Vec<PathBuf>> {
  let entries = match fs::read_dir(path) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(e),
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
