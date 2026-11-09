//! Moving an install's data and logs into another install's, merging with
//! what's there.
//!
//! Journal files are named by instance, so two installs' never collide; the
//! one exception refused up front is a file both have (the same archived
//! day, say), which is left for a person to sort out.

use ::std::{
  fs, io,
  path::{Path, PathBuf},
};

/// Every file under `dir`, relative to it.
fn files(dir: &Path) -> io::Result<Vec<PathBuf>> {
  let mut found = Vec::new();
  let mut pending = vec![PathBuf::new()];
  while let Some(relative) = pending.pop() {
    let entries = match fs::read_dir(dir.join(&relative)) {
      Ok(entries) => entries,
      Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
      Err(e) => return Err(e),
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

/// Files under both `from` and `to`, which a merge would have to overwrite.
pub fn collisions(from: &Path, to: &Path) -> io::Result<Vec<PathBuf>> {
  Ok(
    files(from)?
      .into_iter()
      .filter(|relative| to.join(relative).exists())
      .collect(),
  )
}

/// Moves one file, copying across disks.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
  if let Some(parent) = to.parent() {
    fs::create_dir_all(parent)?;
  }
  if fs::rename(from, to).is_ok() {
    return Ok(());
  }
  fs::copy(from, to)?;
  fs::remove_file(from)
}

/// Removes `dir`'s empty directories, and `dir` if it ends up empty.
fn remove_empty(dir: &Path) {
  if let Ok(entries) = fs::read_dir(dir) {
    for entry in entries.flatten() {
      if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
        remove_empty(&entry.path());
      }
    }
  }
  let _ = fs::remove_dir(dir);
}

/// Moves everything under `from` into `to`, refusing (before moving
/// anything) if a file is in both.
pub fn merge(from: &Path, to: &Path) -> io::Result<()> {
  if same(from, to) || !from.exists() {
    return Ok(());
  }
  let collisions = collisions(from, to)?;
  if !collisions.is_empty() {
    return Err(io::Error::other(format!(
      "both {} and {} have {}; move one of them aside, then try again",
      from.display(),
      to.display(),
      collisions
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
    )));
  }
  for relative in files(from)? {
    move_file(&from.join(&relative), &to.join(&relative))?;
  }
  remove_empty(from);
  Ok(())
}

/// Moves the logs in `from` into `to`, each in front of any of the same
/// name there (which are newer).
pub fn merge_logs(from: &Path, to: &Path) -> io::Result<()> {
  if same(from, to) || !from.exists() {
    return Ok(());
  }
  fs::create_dir_all(to)?;
  for relative in files(from)? {
    let (old, new) = (from.join(&relative), to.join(&relative));
    if new.exists() {
      let mut joined = fs::read(&old)?;
      joined.extend(fs::read(&new)?);
      fs::write(&new, joined)?;
      fs::remove_file(&old)?;
    } else {
      move_file(&old, &new)?;
    }
  }
  remove_empty(from);
  Ok(())
}

/// Whether `a` and `b` are the same directory.
pub fn same(a: &Path, b: &Path) -> bool {
  match (fs::canonicalize(a), fs::canonicalize(b)) {
    (Ok(a), Ok(b)) => a == b,
    _ => a == b,
  }
}
