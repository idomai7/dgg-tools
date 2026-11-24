//! Where journal files live, independent of the record format inside them.

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

/// Kick's journal root under dgg's journal `root`: `<root>/kick`, holding a
/// journal root per channel.
pub fn kick_root(root: &Path) -> PathBuf {
  root.join("kick")
}

/// The journal root for one Kick channel under Kick's journal root
/// `kick_root`: `<kick_root>/<slug>`, laid out like any other.
pub fn channel_root(kick_root: &Path, slug: &str) -> PathBuf {
  kick_root.join(slug)
}

/// The instance that wrote the live file at `path`, and which of its day's
/// files it is (see [`numbered_live_file`]). `<instance>.<n>.journal` is
/// the `n`th when `<instance>.journal` is beside it, as a writer always
/// starts there; otherwise its whole stem is the instance, which may hold
/// dots.
pub fn live_file_instance(path: &Path) -> Option<(String, u32)> {
  let stem = path.file_stem()?.to_str()?;
  if let Some((instance, n)) = stem.rsplit_once('.')
    && !n.is_empty()
    && n.bytes().all(|b| b.is_ascii_digit())
    && let Ok(n) = n.parse::<u32>()
    && n >= 2
    && path
      .with_file_name(format!("{instance}.{EXTENSION}"))
      .exists()
  {
    return Some((instance.into(), n));
  }
  Some((stem.into(), 1))
}

/// Every live journal file, sorted by day, then instance, then its number
/// among the instance's files that day (see [`live_file_instance`]).
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
  files.sort_by_cached_key(|path| {
    (path.parent().map(Path::to_owned), live_file_instance(path))
  });
  Ok(files)
}

/// The backfill journal under journal `root`: a journal root of its own,
/// laid out the same way (`backfill/live/YYYY-MM-DD/`, `backfill/archive/
/// YYYY/MM/`), so everything here works on it too. It holds only what was
/// fetched from other archives (`fetched` and `fetchrange` records), never
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

/// The lock a backfill run holds on `instance` under journal `root` for as
/// long as it may write that instance's files, of any day:
/// `control/<instance>.lock` (see
/// [`Writer::backfill`](crate::Writer::backfill)).
pub fn lock_file(root: &Path, instance: &str) -> PathBuf {
  root.join("control").join(format!("{instance}.lock"))
}

/// The lock a capture holds on `instance` under journal `root` for as long
/// as it may write that instance's live files, of days still open:
/// `control/<instance>.live-lock` (see
/// [`Writer::capture`](crate::Writer::capture)). Apart from
/// [`lock_file`], as it doesn't keep closed days from being compacted.
pub fn live_lock_file(root: &Path, instance: &str) -> PathBuf {
  root.join("control").join(format!("{instance}.live-lock"))
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
