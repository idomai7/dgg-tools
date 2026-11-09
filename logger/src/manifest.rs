//! The journal files a host has, for `sync` to compare against its own.
//!
//! ```text
//! journal-manifest 0.0
//! live/<YYYY-MM-DD>/<instance>.journal <size in bytes>
//! ```
//!
//! One line per file after the format name and `<major>.<minor>` version.
//! Paths are relative to the journal root, `/`-separated, and checked with
//! [`valid_live_path`].
//!
//! While the major is 0 (pre-release) any minor may break, so readers accept
//! only the exact header they write. From 1.0 on, readers will ignore lines
//! they don't recognise, so a minor bump may add kinds of entry; anything
//! else will be a major bump.

use {
  ::journal::{
    layout::{EXTENSION, live_files},
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
}

/// Whether `path` names a live journal file: `live/<YYYY-MM-DD>/<instance>
/// .journal`, nothing more, so it can't reach outside the journal.
pub fn valid_live_path(path: &str) -> bool {
  let mut parts = path.split('/');
  let (Some("live"), Some(day), Some(file), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return false;
  };
  let day_ok = day.len() == 10
    && day.bytes().enumerate().all(|(i, b)| match i {
      4 | 7 => b == b'-',
      _ => b.is_ascii_digit(),
    });
  let instance = file.strip_suffix(&format!(".{EXTENSION}"));
  day_ok && instance.is_some_and(valid_instance)
}

/// Every live file under `journal_root`, with its size.
pub fn list(journal_root: &Path) -> io::Result<Vec<File>> {
  let mut files = Vec::new();
  for path in live_files(journal_root)? {
    let relative = path.strip_prefix(journal_root).unwrap_or(&path);
    let relative = relative.to_string_lossy().replace('\\', "/");
    if valid_live_path(&relative) {
      let size = fs::metadata(&path)?.len();
      files.push(File {
        path: relative,
        size,
      });
    }
  }
  Ok(files)
}

pub fn encode(files: &[File]) -> String {
  let mut text = format!("{HEADER}\n");
  for file in files {
    text += &format!("{} {}\n", file.path, file.size);
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
    let Some((path, size)) = line.split_once(' ') else {
      continue;
    };
    if !valid_live_path(path) {
      continue;
    }
    if size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
      return None;
    }
    files.push(File {
      path: path.into(),
      size: size.parse().ok()?,
    });
  }
  Some(files)
}
