//! Kick: the channels the logger captures, read anonymously through Kick's
//! undocumented endpoints, each journaled in a journal root of its own,
//! `kick/<slug>/`.

pub mod capture;
pub mod channels;
pub mod history;
pub mod index;
pub mod tail;

use {
  crate::{
    index::Error,
    service::{Root, Service},
  },
  ::logger_journal::Variant,
  ::rusqlite::Connection,
  ::std::{
    io,
    path::{Path, PathBuf},
  },
};

/// Kick, as a [`Service`]: a journal root per channel, named by its slug
/// (see [`channels::valid_slug`]).
pub struct Kick;

impl Service for Kick {
  const NAME: &'static str = "kick";
  const API: &'static str = "v0";
  const VARIANT: Variant = Variant::Kick;
  const INDEX: &'static str = index::FILE;

  type Stats = index::Stats;

  fn dir(journal: &Path) -> PathBuf {
    journal.join(Self::NAME)
  }

  /// Every channel with a journal, even one dropped from the list.
  fn roots(dir: &Path) -> io::Result<Vec<Root>> {
    let slugs = channels::journaled(dir)?;
    Ok(
      slugs
        .iter()
        .map(|slug| Root::new::<Self>(dir, &format!("{slug}/"), slug))
        .collect(),
    )
  }

  fn split(path: &str) -> Option<(&str, &str)> {
    let at = path.find('/')?;
    channels::valid_slug(&path[..at]).then(|| (&path[..=at], &path[at + 1..]))
  }

  fn open_index(path: &Path) -> Result<Connection, Error> {
    index::open(path)
  }

  fn index(db: &mut Connection, journal: &Path) -> Result<index::Stats, Error> {
    index::index(db, journal)
  }

  fn waiting(
    db: &Connection,
    journal: &Path,
    at_most: u64,
  ) -> Result<u64, Error> {
    index::waiting(db, journal, at_most)
  }
}
