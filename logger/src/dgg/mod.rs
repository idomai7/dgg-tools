//! dgg: its chat, captured into the journal, indexed, and filled in from
//! other archives where capture missed it.

pub mod backfill;
pub mod capture;
pub mod gaps;
pub mod index;

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

/// dgg, as a [`Service`]: its journal, in `dgg/`, and the backfill journal
/// (see [`backfill_root`](::logger_journal::layout::backfill_root)), at
/// `backfill/` in it.
pub struct Dgg;

impl Service for Dgg {
  const NAME: &'static str = "dgg";
  const API: &'static str = "v0";
  const VARIANT: Variant = Variant::Dgg;
  const INDEX: &'static str = index::FILE;

  type Stats = index::Stats;

  fn dir(journal: &Path) -> PathBuf {
    journal.join(Self::NAME)
  }

  fn roots(dir: &Path) -> io::Result<Vec<Root>> {
    Ok(vec![
      Root::new::<Self>(dir, "", ""),
      Root::new::<Self>(dir, "backfill/", "backfill"),
    ])
  }

  fn split(path: &str) -> Option<(&str, &str)> {
    Some(match path.strip_prefix("backfill/") {
      Some(rest) => ("backfill/", rest),
      None => ("", path),
    })
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
