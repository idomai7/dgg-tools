//! Moving dgg's journals into a directory per channel: what was in `dgg/`
//! into destiny.gg's, `dgg/destiny.gg/`, and its index,
//! `index-dgg.sqlite`, to `index-dgg-destiny.gg.sqlite`. Paths in a
//! channel's directory are as they were in `dgg/`, so the index moves as it
//! is. [`crate::migrate`] runs it as a supervisor starts, under its locks,
//! so nothing indexes, compacts or syncs meanwhile.
//!
//! - Archives, their manifests, `synced-archives` and live files, of dgg's
//!   journal and of its backfill journal, move. A file already at its new
//!   path is left where it was, and the migration fails, for a person to
//!   sort out.
//! - A live file a capture or a backfill run may still write is left where
//!   it is, for a later migrate; nothing reads it there meanwhile. (In an
//!   install there's none: the old supervisor has stopped by then.)
//! - In the old `control/` directories, statuses are removed but for those
//!   of captures still running there, and locks no one holds, then the old
//!   directories once they're empty. Once the old `control/` is gone,
//!   captures stop writing their status there too (see
//!   [`Config::old_control`](super::capture::Config::old_control)).
//! - The index is renamed, with its WAL files, unless there's one at its
//!   new name already.
//!
//! Each step is safe to repeat, so a migration that stopped part way, or
//! left files being written, is finished at a later start.

use {
  crate::{
    capture::Handoff,
    compact::{busy_file, captured},
    control,
    dgg::{Dgg, channels::DESTINY, index},
    service::Service,
    taken,
  },
  ::common::{
    atomic_file,
    fs::{files, move_file, remove_empty_dirs},
    log::Level,
    time::now_ms,
  },
  ::logger_journal::{
    Variant,
    layout::{backfill_root, live_files},
    writer::{capturing, writing},
  },
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// What dgg's directory had before it was by channel, and has no longer.
pub const OLD_LAYOUT: [&str; 4] = ["live", "archive", "control", "backfill"];

/// The index's file before dgg's indexes were by channel.
pub const OLD_INDEX: &str = "index-dgg.sqlite";

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

/// Moves dgg's journals in `<data>/journal`, and its index in `<data>`, to
/// where this build has them (see the [module docs](self)), saying what it
/// does to `say`.
pub fn move_in(
  data: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<Moved> {
  let dir = Dgg::dir(&data.join("journal"));
  let channel = Dgg::channel_dir(&dir, DESTINY);
  let now = now_ms();
  let mut moved = Moved::default();
  // dgg's journal, then its backfill journal.
  let roots = [
    (dir.clone(), channel.clone()),
    (backfill_root(&dir), backfill_root(&channel)),
  ];
  for (from, to) in &roots {
    for relative in files(&from.join("archive"))? {
      let relative = Path::new("archive").join(relative);
      move_file_in(from, to, &relative, dry_run, &mut moved);
    }
    if from.join(taken::PATH).exists() {
      match merge_taken(from, to, dry_run) {
        Ok(()) => moved.files += 1,
        Err(e) => moved.failed.push(format!("{}: {e}", taken::PATH)),
      }
    }
    for file in live_files(from)? {
      if captured(from, &file, now)? || busy_file(from, &file)? {
        moved.left.push(file);
        continue;
      }
      let relative = file.strip_prefix(from).unwrap_or(&file).to_owned();
      move_file_in(from, to, &relative, dry_run, &mut moved);
    }
    if !dry_run {
      tidy_control(from, to, now)?;
    }
  }
  if !dry_run {
    for old in OLD_LAYOUT {
      remove_empty_dirs(&dir.join(old));
    }
  }
  let verb = if dry_run { "would move" } else { "moved" };
  if moved.files > 0 {
    say(
      Level::Info,
      &format!("dgg: {verb} {} files into dgg/{DESTINY}/", moved.files),
    );
  }
  if !moved.left.is_empty() {
    say(
      Level::Warn,
      &format!(
        "dgg: {} live files still being written, left where they were \
         for a later migrate",
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

/// Clears what's done with from journal root `from`'s control directory,
/// whose journals moved to `to`'s: statuses but those of captures still
/// writing there (a fresh one that isn't a copy of one in `to`'s), locks
/// no one holds, and downloads a sync left.
fn tidy_control(from: &Path, to: &Path, now: i64) -> io::Result<()> {
  let old = control::dir(from);
  let copies: Vec<String> = control::read_all(&control::dir(to))?
    .into_iter()
    .map(|status| status.instance)
    .collect();
  let stale_after = Handoff::default().stale_after.as_millis() as i64;
  for status in control::read_all(&old)? {
    let fresh = now - status.heartbeat <= stale_after;
    if !fresh || copies.contains(&status.instance) {
      control::remove(&old, &status.instance)?;
    }
  }
  for relative in files(&old)? {
    let name = relative.to_string_lossy();
    let held =
      match (name.strip_suffix(".live-lock"), name.strip_suffix(".lock")) {
        (Some(instance), _) => capturing(from, instance)?,
        (_, Some(instance)) => writing(from, instance)?,
        _ if name.ends_with(".download") => false,
        _ => continue,
      };
    if !held {
      fs::remove_file(old.join(&relative))?;
    }
  }
  Ok(())
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

/// Renames the index in `data`, and its WAL files, to destiny.gg's, unless
/// there's one there already.
fn move_index(
  data: &Path,
  dry_run: bool,
  moved: &mut Moved,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<()> {
  let file = index::file(DESTINY);
  let (old, new) = (data.join(OLD_INDEX), data.join(&file));
  if !old.exists() {
    return Ok(());
  }
  if new.exists() {
    let why = format!(
      "both {OLD_INDEX} and {file} are there; delete one (an index can be \
       rebuilt)"
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
  say(Level::Info, &format!("index: {verb} {OLD_INDEX} to {file}"));
  Ok(())
}
