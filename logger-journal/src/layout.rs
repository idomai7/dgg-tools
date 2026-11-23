//! Where journal files live, independent of the line format inside them.

use {
  ::common::fs::MissingOk,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// File extension of live journal files.
pub const EXTENSION: &str = "journal";

/// File name suffix of archived (compacted, zstd-compressed) journal files:
/// one per UTC day, at `archive/YYYY/MM/YYYY-MM-DD.journal.zst`.
pub const ARCHIVE_SUFFIX: &str = ".journal.zst";

/// The directory holding the per-day live directories under journal `root`.
pub fn live_dir(root: &Path) -> PathBuf {
  root.join("live")
}

/// The live file one instance writes for one day.
pub fn live_file(root: &Path, day: &str, instance: &str) -> PathBuf {
  numbered_live_file(root, day, instance, 1)
}

/// The `n`th live file one instance writes for one day, counting from 1:
/// `<instance>.journal`, then `<instance>.<n>.journal`. Kick's writer starts
/// the next one when the endpoint changes.
pub fn numbered_live_file(
  root: &Path,
  day: &str,
  instance: &str,
  n: u32,
) -> PathBuf {
  let name = if n == 1 {
    format!("{instance}.{EXTENSION}")
  } else {
    format!("{instance}.{n}.{EXTENSION}")
  };
  live_dir(root).join(day).join(name)
}

/// The journal root for one Kick channel under Kick's journal root
/// `kick_root`: `<kick_root>/<slug>`, laid out like any other.
pub fn channel_root(kick_root: &Path, slug: &str) -> PathBuf {
  kick_root.join(slug)
}

/// Every live journal file, sorted by day then instance.
pub fn live_files(root: &Path) -> io::Result<Vec<PathBuf>> {
  let dir = live_dir(root);
  let mut files = Vec::new();
  let Some(days) = fs::read_dir(&dir).missing_ok()? else {
    return Ok(files);
  };
  for day in days {
    let day = day?;
    if !day.file_type()?.is_dir() {
      continue;
    }
    for file in fs::read_dir(day.path())? {
      let path = file?.path();
      if path
        .extension()
        .is_some_and(|extension| extension == EXTENSION)
      {
        files.push(path);
      }
    }
  }
  files.sort();
  Ok(files)
}

/// The backfill journal under journal `root`: a journal root of its own,
/// laid out the same way (`backfill/live/YYYY-MM-DD/`, `backfill/archive/
/// YYYY/MM/`), so everything here works on it too. It holds only what was
/// fetched from other archives (`fetched` and `fetchrange` lines), never
/// anything captured natively: everything under it is non-canonical.
pub fn backfill_root(root: &Path) -> PathBuf {
  root.join("backfill")
}

/// The backfill file one run writes for one day. Unlike live files, the day
/// is that of the messages inside, not of when they were fetched.
pub fn backfill_file(root: &Path, day: &str, instance: &str) -> PathBuf {
  live_file(&backfill_root(root), day, instance)
}

/// Every uncompacted backfill file, sorted by day then instance.
pub fn backfill_files(root: &Path) -> io::Result<Vec<PathBuf>> {
  live_files(&backfill_root(root))
}

/// The lock a writer holds on `instance` under journal `root` for as long
/// as it may write that instance's files: `control/<instance>.lock`. Only
/// backfill runs take one (see [`Writer::backfill`](crate::Writer::backfill)),
/// since only they write to days that are already closed.
pub fn lock_file(root: &Path, instance: &str) -> PathBuf {
  root.join("control").join(format!("{instance}.lock"))
}

/// The directory holding archived days under journal `root`.
pub fn archive_dir(root: &Path) -> PathBuf {
  root.join("archive")
}

/// The archive directory for the month containing `day` (`YYYY-MM-DD`):
/// `archive/YYYY/MM`. Each holds that month's archives and its
/// [manifest](crate::archive).
pub fn archive_month_dir(root: &Path, day: &str) -> PathBuf {
  archive_dir(root).join(&day[..4]).join(&day[5..7])
}

/// The archive for `day` (`YYYY-MM-DD`).
pub fn archive_file(root: &Path, day: &str) -> PathBuf {
  archive_month_dir(root, day).join(format!("{day}{ARCHIVE_SUFFIX}"))
}

/// The live directory for `day`.
pub fn live_day_dir(root: &Path, day: &str) -> PathBuf {
  live_dir(root).join(day)
}
