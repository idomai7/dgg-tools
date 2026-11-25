//! The services the logger captures, each with journals and an index of
//! its own: [`Dgg`](crate::dgg::Dgg) and [`Kick`](crate::kick::Kick). What
//! they share is written once, generic over [`Service`], and called for
//! each in turn, so a new service is a module implementing it.
//!
//! A service's journals are in a directory of its own under the journal
//! directory, as one or more journal roots, each laid out as any (see
//! [`layout`](::logger_journal::layout)).
//!
//! A service's journals are by channel, each channel's in a directory of its
//! own in the service's, holding one or more of its journal roots. Paths in
//! a channel's manifest and its routes are relative to the channel's
//! directory.
//!
//! A service has one or more indexes (see [`Service::indexes`]), each of
//! the journals in one directory, its paths relative to it: a channel's
//! each.

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
  /// Its name: its routes' first segment (`/<name>/<api>/...`), its
  /// index's flag (`--rebuild-<name>`), and how what's logged about it
  /// starts.
  const NAME: &'static str;
  /// Its API's version, its routes' second segment. `v0` is unstable: it
  /// may change incompatibly from one build to the next, until a stable
  /// `v1` is decided on.
  const API: &'static str;
  /// Its journals' variant.
  const VARIANT: Variant;
  /// What an index round did.
  type Stats: Tally;

  /// Its directory, under the journal directory `journal`.
  fn dir(journal: &Path) -> PathBuf;

  /// Its journal roots, in its directory `dir`, in order.
  fn roots(dir: &Path) -> io::Result<Vec<Root>>;

  /// Whether `name` could name one of its channels: a directory's name,
  /// and a route's segment.
  fn valid_channel(name: &str) -> bool;

  /// The channels with journals in its directory `dir`, sorted.
  fn channels(dir: &Path) -> io::Result<Vec<String>>;

  /// Channel `name`'s directory, in its directory `dir`.
  fn channel_dir(dir: &Path, name: &str) -> PathBuf;

  /// Channel `name`'s journal roots, in the channel's directory `dir`, in
  /// order, their prefixes relative to it.
  fn channel_roots(dir: &Path, name: &str) -> Vec<Root>;

  /// The prefix of the journal root `path`, relative to a channel's
  /// directory, would be under (see [`Root::prefix`]), and the rest of it;
  /// `None` if it can't be under any.
  fn split(path: &str) -> Option<(&str, &str)>;

  /// Opens its index at `path`, creating it if needed.
  fn open_index(path: &Path) -> Result<Connection, Error>;

  /// Its indexes, of the journals in its directory `dir`.
  fn indexes(dir: &Path) -> io::Result<Vec<Indexed>>;

  /// Indexes everything in the journals in `dir`, one of its indexes'
  /// (see [`Indexed::dir`]), that isn't indexed yet.
  fn index(db: &mut Connection, dir: &Path) -> Result<Self::Stats, Error>;

  /// How many records in the live files in `dir`, one of its indexes',
  /// aren't indexed yet, counting up to `at_most`. Archives aren't counted:
  /// a newly archived day is mostly records already indexed from its live
  /// files.
  fn waiting(db: &Connection, dir: &Path, at_most: u64) -> Result<u64, Error>;
}

/// One of a service's indexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexed {
  /// What it's called in what's logged: the service's name, then its
  /// channel's, if it's one channel's.
  pub name: String,
  /// Its file, in the data directory.
  pub file: String,
  /// The directory whose journals it indexes, its paths relative to it.
  pub dir: PathBuf,
}

/// Where `S`'s routes start: `/<name>/<api>`.
pub fn routes<S: Service>() -> String {
  format!("/{}/{}", S::NAME, S::API)
}

/// One of a service's journal roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
  /// What it's called in what's logged: the service's name, then which
  /// root it is, if it has more than one kind.
  pub name: String,
  /// How its files' paths, relative to the service's directory (or, for a
  /// channel's root, the channel's), start: empty, or a directory's name
  /// and `/`.
  pub prefix: String,
  /// Its directory: the service's (or channel's), joined with the prefix.
  pub dir: PathBuf,
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
    }
  }

  /// A journal root at `dir` on its own, of no service.
  pub fn at(dir: impl Into<PathBuf>) -> Self {
    Self {
      name: String::new(),
      prefix: String::new(),
      dir: dir.into(),
    }
  }

  /// Its live files, sorted by their paths.
  pub fn live_files(&self) -> io::Result<Vec<PathBuf>> {
    let mut files = live_files(&self.dir)?;
    files.sort_by_cached_key(|file| self.path(file));
    Ok(files)
  }

  /// The path of `file`, in this root, relative to the service's (or
  /// channel's) directory, `/`-separated.
  pub fn path(&self, file: &Path) -> String {
    let relative = file.strip_prefix(&self.dir).unwrap_or(file);
    let relative = relative.to_string_lossy().replace('\\', "/");
    format!("{}{relative}", self.prefix)
  }
}
