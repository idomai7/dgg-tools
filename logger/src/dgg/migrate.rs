//! Moving dgg's journals to a new layout, while capture keeps running:
//! [`crate::migrate`] runs [`move_in`] as a supervisor starts, under its
//! locks, so nothing indexes, compacts or syncs meanwhile. No move is under
//! way now; this is where the next goes.
//!
//! The last, into a directory per channel (`dgg/` into `dgg/destiny.gg/`,
//! and `index-dgg.sqlite` to `index-dgg-destiny.gg.sqlite`), was built as
//! below; its code is in this file's history (`git log -- <this file>`),
//! from commit 4869223 until it was removed, once every host had moved.
//!
//! - Each file is moved by a rename within the volume; one already at its
//!   new path is left where it was, and the migration fails, for a person
//!   to sort out.
//! - A live file a capture or a backfill run may still write is left where
//!   it is, for a later migrate; nothing reads it there meanwhile. (In an
//!   install there's none: the old supervisor has stopped by then.)
//! - For an old capture to hand off, the new one also heartbeats into the
//!   old `control/` while it's there. Once nothing in it is fresh, migrate
//!   removes it, and the heartbeat stops.
//! - The index is renamed, its WAL files first, unless there's one at its
//!   new name already.
//! - Each step is safe to repeat, so a migration that stopped part way, or
//!   left files being written, is finished at a later start.
//! - What's left of the old layout once every host has moved is added to
//!   [`crate::migrate::OLD_LAYOUT`], warned of and never read.

use {
  ::common::log::Level,
  ::std::{
    io,
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

/// Moves dgg's journals in `<data>/journal`, and its indexes in `<data>`,
/// to where this build has them, saying what it does to `say`: nothing,
/// with no move under way (see the [module docs](self)).
pub fn move_in(
  _data: &Path,
  _dry_run: bool,
  _say: &mut dyn FnMut(Level, &str),
) -> io::Result<Moved> {
  Ok(Moved::default())
}
