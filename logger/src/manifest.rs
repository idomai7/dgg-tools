//! The journal files a host has, for `sync` to compare against its own.
//!
//! ```text
//! journal-manifest 0.0
//! live/<YYYY-MM-DD>/<instance>.journal <size in bytes>
//! backfill/live/<YYYY-MM-DD>/<instance>.journal <size in bytes>
//! archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst <size in bytes> <sha-256>
//! backfill/archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst <size> <sha-256>
//! ```
//!
//! One line per file after the format name and `<major>.<minor>` version:
//! the live files, then the backfill journal's, then the archives of both,
//! as their monthly manifests describe them. Paths are relative to the
//! journal root, `/`-separated, and checked with [`valid_live_path`] and
//! [`archive_path`]. (Older readers skip the lines with paths they don't
//! accept.)
//!
//! While the major is 0 (pre-release) any minor may break, so readers accept
//! only the exact header they write. From 1.0 on, readers will ignore lines
//! they don't recognise, so a minor bump may add kinds of entry; anything
//! else will be a major bump.

use {
  ::logger_journal::{
    archive::{self, valid_day},
    layout::{
      ARCHIVE_SUFFIX, EXTENSION, backfill_files, backfill_root, live_files,
    },
    record::v0::valid_instance,
  },
  ::std::{fs, io, path::Path},
};

/// The header line this crate writes and reads.
pub const HEADER: &str = "journal-manifest 0.0";

/// One file and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
  /// Relative to the journal root, `/`-separated.
  pub path: String,
  pub size: u64,
  /// For archives: the SHA-256 of the file, lowercase hex.
  pub sha256: Option<String>,
}

/// Whether `path` names a live journal file: `live/<YYYY-MM-DD>/<instance>
/// .journal`, or the same under `backfill/`, nothing more, so it can't reach
/// outside the journal.
pub fn valid_live_path(path: &str) -> bool {
  let path = path.strip_prefix("backfill/").unwrap_or(path);
  let mut parts = path.split('/');
  let (Some("live"), Some(day), Some(file), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return false;
  };
  let instance = file.strip_suffix(&format!(".{EXTENSION}"));
  valid_day(day) && instance.is_some_and(valid_instance)
}

/// The day `path` archives, and whether it's the backfill journal's, if it
/// names an archive: `archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst`, its
/// month directories matching its day, or the same under `backfill/`.
pub fn archive_path(path: &str) -> Option<(&str, bool)> {
  let (path, backfill) = match path.strip_prefix("backfill/") {
    Some(path) => (path, true),
    None => (path, false),
  };
  let mut parts = path.split('/');
  let (Some("archive"), Some(year), Some(month), Some(file), None) = (
    parts.next(),
    parts.next(),
    parts.next(),
    parts.next(),
    parts.next(),
  ) else {
    return None;
  };
  let day = file.strip_suffix(ARCHIVE_SUFFIX)?;
  (valid_day(day) && year == &day[..4] && month == &day[5..7])
    .then_some((day, backfill))
}

fn valid_sha256(text: &str) -> bool {
  text.len() == 64
    && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Every live file under `journal_root`, then every backfill one, then the
/// archives of both, with their sizes.
pub fn list(journal_root: &Path) -> io::Result<Vec<File>> {
  let mut files = Vec::new();
  let paths = live_files(journal_root)?
    .into_iter()
    .chain(backfill_files(journal_root)?);
  for path in paths {
    let relative = path.strip_prefix(journal_root).unwrap_or(&path);
    let relative = relative.to_string_lossy().replace('\\', "/");
    if valid_live_path(&relative) {
      let size = fs::metadata(&path)?.len();
      files.push(File {
        path: relative,
        size,
        sha256: None,
      });
    }
  }
  let backfill = backfill_root(journal_root);
  for (root, prefix) in [(journal_root, ""), (backfill.as_path(), "backfill/")]
  {
    for archive in archive::all(root)? {
      let day = &archive.day;
      files.push(File {
        path: format!(
          "{prefix}archive/{}/{}/{day}{ARCHIVE_SUFFIX}",
          &day[..4],
          &day[5..7]
        ),
        size: archive.compressed,
        sha256: Some(archive.sha256),
      });
    }
  }
  Ok(files)
}

pub fn encode(files: &[File]) -> String {
  let mut text = format!("{HEADER}\n");
  for file in files {
    text += &format!("{} {}", file.path, file.size);
    if let Some(sha256) = &file.sha256 {
      text += &format!(" {sha256}");
    }
    text.push('\n');
  }
  text
}

/// Parses a manifest, or `None` if it isn't one this crate can read.
pub fn decode(text: &str) -> Option<Vec<File>> {
  let mut lines = text.strip_suffix('\n')?.split('\n');
  if lines.next()? != HEADER {
    return None;
  }
  let mut files = Vec::new();
  for line in lines {
    let fields: Vec<_> = line.split(' ').collect();
    let (path, size, sha256) = match fields[..] {
      [path, size] if valid_live_path(path) => (path, size, None),
      [path, size, sha256] if archive_path(path).is_some() => {
        if !valid_sha256(sha256) {
          return None;
        }
        (path, size, Some(sha256.to_owned()))
      }
      _ => continue,
    };
    if size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
      return None;
    }
    files.push(File {
      path: path.into(),
      size: size.parse().ok()?,
      sha256,
    });
  }
  Some(files)
}
