//! Where journal files live, independent of the line format inside them.

use ::std::{
  fs, io,
  path::{Path, PathBuf},
};

/// File extension of live journal files.
pub const EXTENSION: &str = "journal";

/// File name suffix of archived (compacted, zstd-compressed) journal files:
/// one per UTC day, at `archive/YYYY/MM/YYYY-MM-DD.journal.zst`.
pub const ARCHIVE_SUFFIX: &str = ".journal.zst";

/// `YYYY-MM-DD` of a unix ms timestamp, in UTC.
pub fn utc_day(ms: i64) -> String {
  // Howard Hinnant's civil_from_days.
  let z = ms.div_euclid(86_400_000) + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z - era * 146_097;
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = doy - (153 * mp + 2) / 5 + 1;
  let m = if mp < 10 { mp + 3 } else { mp - 9 };
  let y = yoe + era * 400 + i64::from(m <= 2);
  format!("{y:04}-{m:02}-{d:02}")
}

/// The directory holding the per-day live directories under journal `root`.
pub fn live_dir(root: &Path) -> PathBuf {
  root.join("live")
}

/// The live file one instance writes for one day.
pub fn live_file(root: &Path, day: &str, instance: &str) -> PathBuf {
  live_dir(root)
    .join(day)
    .join(format!("{instance}.{EXTENSION}"))
}

/// Every live journal file, sorted by day then instance.
pub fn live_files(root: &Path) -> io::Result<Vec<PathBuf>> {
  let mut files = Vec::new();
  let days = match fs::read_dir(live_dir(root)) {
    Ok(days) => days,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(files),
    Err(e) => return Err(e),
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
