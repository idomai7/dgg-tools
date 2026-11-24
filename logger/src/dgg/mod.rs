//! dgg: its chat, captured into the journal, indexed, and filled in from
//! other archives where capture missed it.

pub mod backfill;
pub mod capture;
pub mod gaps;
pub mod index;

use {
  crate::{index::Error, service::Service},
  ::logger_journal::Variant,
  ::rusqlite::Connection,
  ::std::path::Path,
};

/// dgg, as a [`Service`].
pub struct Dgg;

impl Service for Dgg {
  const NAME: &'static str = "dgg";
  const VARIANT: Variant = Variant::Dgg;
  const INDEX: &'static str = index::FILE;

  type Stats = index::Stats;

  fn open_index(path: &Path) -> Result<Connection, Error> {
    index::open(path)
  }

  fn index(
    db: &mut Connection,
    journal_root: &Path,
  ) -> Result<index::Stats, Error> {
    index::index(db, journal_root)
  }

  fn waiting(
    db: &Connection,
    journal_root: &Path,
    at_most: u64,
  ) -> Result<u64, Error> {
    index::waiting(db, journal_root, at_most)
  }
}
