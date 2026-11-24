//! Moving dgg's journals into a directory of their own, `journal/dgg/`,
//! and its index to `index-dgg.sqlite`, while a capture from before may
//! still be writing. [`crate::migrate`] runs it as a supervisor starts,
//! under its locks, so nothing indexes, compacts or syncs meanwhile.
//!
//! - Archives, their manifests and `synced-archives`, of dgg's journal and
//!   of the backfill journal, move. A file already at its new path is left
//!   where it was, and the migration fails, for a person to sort out.
//! - Live files move, unless they're still being written: a capture's whose
//!   status in the old `control/` is fresh, and isn't a copy (a capture from
//!   this build writes its status in both, see
//!   [`Config::legacy_control`](super::capture::Config::legacy_control)), or
//!   a backfill run's that holds its lock. Those are read where they are
//!   (see [`Root::legacy`](crate::service::Root::legacy)) until compaction merges them away.
//! - Old statuses that are stale or copies are removed, then the old
//!   directories once they're empty: once the old `control/` is gone, a
//!   capture from this build stops writing there.
//! - The index moves, with its WAL files, unless there's one at its new
//!   name already.
//!
//! Each step is safe to repeat, so a migration that stopped part way, or
//! left files being written, is finished at the next start.

use {
  crate::{
    capture::Handoff,
    compact::busy_file,
    control,
    dgg::{Dgg, index},
    service::Service,
    taken,
  },
  ::common::{
    atomic_file,
    fs::{files, move_file, remove_empty_dirs},
    log::Level,
    time::now_ms,
  },
  ::logger_journal::{Variant, layout::live_files},
  ::std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
  },
};

/// What [`move_in`] did, or with a dry run would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
  /// Files moved.
  pub files: u64,
  /// Live files left where they were, still being written.
  pub left: Vec<PathBuf>,
  /// What couldn't be moved, with why.
  pub failed: Vec<String>,
}

/// Moves dgg's journals in `<data>/journal`, and its index in `<data>`,
/// to where this build has them (see the [module docs](self)), saying what
/// it does to `say`.
pub fn move_in(
  data: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<Moved> {
  let journal = data.join("journal");
  let dir = Dgg::dir(&journal);
  let old_control = control::dir(&journal);
  let mut moved = Moved::default();
  let writing = writing_captures(&journal)?;
  for root in Dgg::roots(&dir)? {
    let Some(legacy) = &root.legacy else {
      continue;
    };
    for relative in files(&legacy.join("archive"))? {
      let relative = Path::new("archive").join(relative);
      move_file_in(legacy, &root.dir, &relative, dry_run, &mut moved);
    }
    if legacy.join(taken::PATH).exists() {
      match merge_taken(legacy, &root.dir, dry_run) {
        Ok(()) => moved.files += 1,
        Err(e) => moved.failed.push(format!("{}: {e}", taken::PATH)),
      }
    }
    for file in live_files(legacy)? {
      let instance = instance(&file);
      if writing.contains(instance) || busy_file(legacy, &file)? {
        moved.left.push(file);
        continue;
      }
      let relative = file.strip_prefix(legacy).unwrap_or(&file);
      move_file_in(legacy, &root.dir, relative, dry_run, &mut moved);
    }
    // Locks of backfill runs that have finished.
    for lock in files(&control::dir(legacy))? {
      let path = control::dir(legacy).join(&lock);
      let name = lock.to_string_lossy();
      if let Some(instance) = name.strip_suffix(".lock")
        && !busy_file(legacy, Path::new(&format!("{instance}.journal")))?
        && !dry_run
      {
        fs::remove_file(path)?;
      }
    }
  }
  for status in control::read_all(&old_control)? {
    if !writing.contains(&status.instance) && !dry_run {
      control::remove(&old_control, &status.instance)?;
    }
  }
  if !dry_run {
    for old in ["live", "archive", "control", "backfill"] {
      remove_empty_dirs(&journal.join(old));
    }
  }
  let verb = if dry_run { "would move" } else { "moved" };
  if moved.files > 0 {
    say(
      Level::Info,
      &format!("dgg: {verb} {} files into dgg/", moved.files),
    );
  }
  if !moved.left.is_empty() {
    say(
      Level::Warn,
      &format!(
        "dgg: {} live files still being written, left where they were \
         (read there until compacted)",
        moved.left.len()
      ),
    );
  }
  for why in &moved.failed {
    say(Level::Error, &format!("dgg: {why}"));
  }
  move_index(data, dry_run, &mut moved, say)?;
  Ok(moved)
}

/// The captures whose live files in the journal directory `journal` are
/// still being written: those with a fresh status in its old `control/`
/// that isn't a copy of one in dgg's own.
fn writing_captures(journal: &Path) -> io::Result<HashSet<String>> {
  let copies: HashSet<String> =
    control::read_all(&control::dir(&Dgg::dir(journal)))?
      .into_iter()
      .map(|status| status.instance)
      .collect();
  let stale_after = Handoff::default().stale_after.as_millis() as i64;
  let now = now_ms();
  Ok(
    control::read_all(&control::dir(journal))?
      .into_iter()
      .filter(|status| {
        !copies.contains(&status.instance)
          && now - status.heartbeat <= stale_after
      })
      .map(|status| status.instance)
      .collect(),
  )
}

/// The instance that wrote the live file at `path`: `<instance>.journal`,
/// or `<instance>.<n>.journal`.
fn instance(path: &Path) -> &str {
  let stem = path
    .file_stem()
    .and_then(|stem| stem.to_str())
    .unwrap_or("");
  match stem.rsplit_once('.') {
    Some((instance, n))
      if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
    {
      instance
    }
    _ => stem,
  }
}

/// Moves `relative` from `from` to `to`, unless something is there
/// already, noting it in `moved`.
fn move_file_in(
  from: &Path,
  to: &Path,
  relative: &Path,
  dry_run: bool,
  moved: &mut Moved,
) {
  let (old, new) = (from.join(relative), to.join(relative));
  if new.exists() {
    moved.failed.push(format!(
      "both {} and {} are there; move one of them aside",
      old.display(),
      new.display()
    ));
    return;
  }
  if !dry_run && let Err(e) = move_file(&old, &new) {
    moved.failed.push(format!("{}: {e}", old.display()));
    return;
  }
  moved.files += 1;
}

/// Adds what `from`'s `synced-archives` lists to `to`'s, then removes
/// `from`'s.
fn merge_taken(from: &Path, to: &Path, dry_run: bool) -> io::Result<()> {
  let mut taken = taken::read(to, Variant::Dgg)?;
  for entry in taken::read(from, Variant::Dgg)? {
    if !taken.contains(&entry) {
      taken.push(entry);
    }
  }
  if dry_run {
    return Ok(());
  }
  atomic_file::replace(
    &to.join(taken::PATH),
    taken::encode(Variant::Dgg, &taken),
  )?;
  fs::remove_file(from.join(taken::PATH))
}

/// Renames the index in `data`, and its WAL files, to its new name, unless
/// there's one there already.
fn move_index(
  data: &Path,
  dry_run: bool,
  moved: &mut Moved,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<()> {
  let (old, new) = (data.join(index::LEGACY_FILE), data.join(index::FILE));
  if !old.exists() {
    return Ok(());
  }
  if new.exists() {
    let why = format!(
      "both {} and {} are there; delete one (an index can be rebuilt)",
      index::LEGACY_FILE,
      index::FILE
    );
    say(Level::Error, &format!("index: {why}"));
    moved.failed.push(format!("index: {why}"));
    return Ok(());
  }
  let verb = if dry_run { "would rename" } else { "renamed" };
  if !dry_run {
    // The WAL files first: if it stops part way, the index is still where
    // the next try finds it.
    for suffix in ["-wal", "-shm", ""] {
      let with = |path: &Path| {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        PathBuf::from(file)
      };
      if with(&old).exists()
        && let Err(e) = fs::rename(with(&old), with(&new))
      {
        let why = format!("can't rename {}: {e}", with(&old).display());
        say(Level::Error, &format!("index: {why}"));
        moved.failed.push(format!("index: {why}"));
        return Ok(());
      }
    }
  }
  say(
    Level::Info,
    &format!("index: {verb} {} to {}", index::LEGACY_FILE, index::FILE),
  );
  Ok(())
}
