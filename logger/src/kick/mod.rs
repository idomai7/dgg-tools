//! Kick: the channels the logger captures, read anonymously through Kick's
//! undocumented endpoints, each journaled under `kick/<slug>/`.

pub mod capture;
pub mod channels;
pub mod history;
pub mod index;
pub mod tail;

use {
  crate::{index::Error, service::Service},
  ::logger_journal::Variant,
  ::rusqlite::Connection,
  ::std::path::Path,
};

/// Kick, as a [`Service`].
pub struct Kick;

impl Service for Kick {
  const NAME: &'static str = "kick";
  const VARIANT: Variant = Variant::Kick;
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
