//! Moving an install's data and logs into another install's, merging with
//! what's there.
//!
//! Journal files are named by instance, so two installs' never collide; the
//! one exception refused up front is a file both have (the same archived
//! day, say), which is left for a person to sort out.

use {
  ::common::fs::{MissingOk, files, move_file, remove_empty_dirs, same},
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// Files under both `from` and `to`, which a merge would have to overwrite.
pub fn collisions(from: &Path, to: &Path) -> io::Result<Vec<PathBuf>> {
  Ok(
    files(from)?
      .into_iter()
      .filter(|relative| to.join(relative).exists())
      .collect(),
  )
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
  remove_empty_dirs(from);
  Ok(())
}

/// Renames the log `old` in `logs` to `new`, rotated ones too
/// (`<old>.<n>.log`), each in front of any of the new name there (which is
/// newer): for a program renamed.
pub fn rename_logs(logs: &Path, old: &str, new: &str) -> io::Result<()> {
  let Some(entries) = fs::read_dir(logs).missing_ok()? else {
    return Ok(());
  };
  for entry in entries {
    let name = entry?.file_name().to_string_lossy().into_owned();
    let Some(rest) = name.strip_prefix(old) else {
      continue;
    };
    let rotated = rest
      .strip_prefix('.')
      .and_then(|rest| rest.strip_suffix(".log"))
      .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if rest != ".log" && !rotated {
      continue;
    }
    let (from, to) = (logs.join(&name), logs.join(format!("{new}{rest}")));
    if to.exists() {
      let mut joined = fs::read(&from)?;
      joined.extend(fs::read(&to)?);
      fs::write(&to, joined)?;
      fs::remove_file(&from)?;
    } else {
      fs::rename(&from, &to)?;
    }
  }
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
  remove_empty_dirs(from);
  Ok(())
}
