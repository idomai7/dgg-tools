//! The services the logger captures, each with journals and an index of
//! its own: [`Dgg`](crate::dgg::Dgg) and [`Kick`](crate::kick::Kick). What
//! they share is written once, generic over [`Service`], and called for
//! each in turn, so a new service is a module implementing it.
//!
//! A service's journals are in a directory of its own under the journal
//! directory, as one or more journal roots, each laid out as any (see
//! [`layout`](::logger_journal::layout)). Paths in a service's manifest, its
//! routes and its index are relative to its directory.
//!
//! dgg's journals were once the journal directory itself. A host migrated
//! while a capture from before kept writing (see [`crate::migrate`]) can
//! have that capture's live files where they were, so a root can have a
//! [legacy](Root::legacy) directory whose live files count as its own,
//! until every host has migrated and compacted them away.

use {
  crate::index::{Error, Tally},
  ::logger_journal::{Variant, layout::live_files},
  ::rusqlite::Connection,
  ::std::{
    io,
    path::{Path, PathBuf},
  },
};

pub trait Service: 'static {
  /// Its name: its routes' first segment (`/<name>/...`), its index's
  /// flag (`--rebuild-<name>`), and how what's logged about it starts.
  const NAME: &'static str;
  /// Its journals' variant.
  const VARIANT: Variant;
  /// Its index's file, in the data directory.
  const INDEX: &'static str;

  /// What an index round did.
  type Stats: Tally;

  /// Its directory, under the journal directory `journal`.
  fn dir(journal: &Path) -> PathBuf;

  /// Its journal roots, in its directory `dir`, in order.
  fn roots(dir: &Path) -> io::Result<Vec<Root>>;

  /// The prefix of the journal root `path`, relative to its directory,
  /// would be under (see [`Root::prefix`]), and the rest of it; `None` if
  /// it can't be under any.
  fn split(path: &str) -> Option<(&str, &str)>;

  /// Where the journal root at `prefix`, in its directory `dir`, was
  /// before the service had a directory of its own, if it was anywhere
  /// else (see [`Root::legacy`]).
  fn legacy(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let _ = (dir, prefix);
    None
  }

  /// Opens its index at `path`, creating it if needed.
  fn open_index(path: &Path) -> Result<Connection, Error>;

  /// Indexes everything in its journals, under the journal directory
  /// `journal`, that isn't indexed yet.
  fn index(db: &mut Connection, journal: &Path) -> Result<Self::Stats, Error>;

  /// How many records in its live files aren't indexed yet, counting up to
  /// `at_most`. Archives aren't counted: a newly archived day is mostly
  /// records already indexed from its live files.
  fn waiting(
    db: &Connection,
    journal: &Path,
    at_most: u64,
  ) -> Result<u64, Error>;
}

/// One of a service's journal roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
  /// What it's called in what's logged: the service's name, then which
  /// root it is, if it has more than one kind.
  pub name: String,
  /// How its files' paths, relative to the service's directory, start:
  /// empty, or a directory's name and `/`.
  pub prefix: String,
  /// Its directory: the service's, joined with the prefix.
  pub dir: PathBuf,
  /// Where it was before its service had a directory of its own, if
  /// anywhere else: live files still there (a capture from before kept
  /// writing them) count as its own, under the same paths. Nothing else
  /// there is read: `migrate` moved it.
  pub legacy: Option<PathBuf>,
}

impl Root {
  /// The root at `prefix` in service `S`'s directory `dir`, called `name`.
  pub fn new<S: Service>(dir: &Path, prefix: &str, name: &str) -> Self {
    Self {
      name: match name {
        "" => S::NAME.to_owned(),
        name => format!("{} {name}", S::NAME),
      },
      prefix: prefix.to_owned(),
      dir: match prefix.strip_suffix('/') {
        Some(directory) => dir.join(directory),
        None => dir.to_owned(),
      },
      legacy: S::legacy(dir, prefix),
    }
  }

  /// A journal root at `dir` on its own, of no service.
  pub fn at(dir: impl Into<PathBuf>) -> Self {
    Self {
      name: String::new(),
      prefix: String::new(),
      dir: dir.into(),
      legacy: None,
    }
  }

  /// Its directory, then its legacy one, if it has one.
  pub fn dirs(&self) -> impl Iterator<Item = &Path> {
    [Some(self.dir.as_path()), self.legacy.as_deref()]
      .into_iter()
      .flatten()
  }

  /// Its live files, sorted by their paths: those in its directory and
  /// those in its legacy one.
  pub fn live_files(&self) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for dir in self.dirs() {
      files.extend(live_files(dir)?);
    }
    files.sort_by_cached_key(|file| self.path(file));
    Ok(files)
  }

  /// The path of `file`, in this root (or its legacy directory), relative
  /// to the service's directory, `/`-separated.
  pub fn path(&self, file: &Path) -> String {
    let relative = self
      .dirs()
      .find_map(|dir| file.strip_prefix(dir).ok())
      .unwrap_or(file);
    let relative = relative.to_string_lossy().replace('\\', "/");
    format!("{}{relative}", self.prefix)
  }
}

/// Where the file at `path`, relative to `S`'s directory `dir`, is: in its
/// root's legacy directory if it's a live file still there, else in `dir`.
pub fn locate<S: Service>(dir: &Path, path: &str) -> PathBuf {
  if let Some((prefix, rest)) = S::split(path)
    && rest.starts_with("live/")
    && let Some(legacy) = S::legacy(dir, prefix)
    && legacy.join(rest).exists()
  {
    return legacy.join(rest);
  }
  dir.join(path)
}
