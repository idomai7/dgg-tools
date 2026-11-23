//! Files and directories, beyond `std::fs`.
//!
//! - A missing file as nothing there: [`MissingOk::missing_ok`].
//! - Every file under a directory: [`files`].
//! - Moving a file, across disks too: [`move_file`].
//! - Removing the empty directories left behind: [`remove_empty_dirs`].
//! - Whether two paths are the same place: [`same`].

use ::std::{
  fs, io,
  path::{Path, PathBuf},
};

/// A missing file or directory as `None`, for when it only means nothing
/// is there yet, or any more.
pub trait MissingOk<T> {
  /// `None` for [`io::ErrorKind::NotFound`]; any other error stays one.
  fn missing_ok(self) -> io::Result<Option<T>>;
}

impl<T> MissingOk<T> for io::Result<T> {
  fn missing_ok(self) -> io::Result<Option<T>> {
    match self {
      Ok(value) => Ok(Some(value)),
      Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
      Err(e) => Err(e),
    }
  }
}

/// Every file under `dir`, relative to it, sorted; none if it's missing.
pub fn files(dir: &Path) -> io::Result<Vec<PathBuf>> {
  let mut found = Vec::new();
  let mut pending = vec![PathBuf::new()];
  while let Some(relative) = pending.pop() {
    let Some(entries) = fs::read_dir(dir.join(&relative)).missing_ok()? else {
      continue;
    };
    for entry in entries {
      let entry = entry?;
      let path = relative.join(entry.file_name());
      if entry.file_type()?.is_dir() {
        pending.push(path);
      } else {
        found.push(path);
      }
    }
  }
  found.sort();
  Ok(found)
}

/// Moves a file, creating `to`'s directory if need be. Across disks, where
/// a rename can't, it's copied and then removed.
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
  if let Some(dir) = to.parent().filter(|dir| !dir.as_os_str().is_empty()) {
    fs::create_dir_all(dir)?;
  }
  if fs::rename(from, to).is_ok() {
    return Ok(());
  }
  fs::copy(from, to)?;
  fs::remove_file(from)
}

/// Removes the empty directories under `dir`, and `dir` if it ends up
/// empty. What can't be removed is left as it is.
pub fn remove_empty_dirs(dir: &Path) {
  if let Ok(entries) = fs::read_dir(dir) {
    for entry in entries.flatten() {
      if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
        remove_empty_dirs(&entry.path());
      }
    }
  }
  let _ = fs::remove_dir(dir);
}

/// Whether `a` and `b` are the same place, as far as the OS can tell; if
/// either doesn't exist, whether they're written the same.
pub fn same(a: &Path, b: &Path) -> bool {
  match (fs::canonicalize(a), fs::canonicalize(b)) {
    (Ok(a), Ok(b)) => a == b,
    _ => a == b,
  }
}
