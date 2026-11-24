//! The services the logger captures, each with journals and an index of
//! its own: [`Dgg`](crate::dgg::Dgg) and [`Kick`](crate::kick::Kick). What
//! they share is written once, generic over [`Service`], and called for
//! each in turn, so a new service is a module implementing it.

use {
  crate::index::{Error, Tally},
  ::logger_journal::Variant,
  ::rusqlite::Connection,
  ::std::path::Path,
};

pub trait Service {
  /// Its name: in its index's flags (`--rebuild-<name>`), and in what's
  /// logged about it.
  const NAME: &'static str;
  /// Its journals' variant.
  const VARIANT: Variant;
  /// Its index's file, in the data directory.
  const INDEX: &'static str;

  /// What an index round did.
  type Stats: Tally;

  /// Opens its index at `path`, creating it if needed.
  fn open_index(path: &Path) -> Result<Connection, Error>;

  /// Indexes everything in its journals, under `journal_root`, that isn't
  /// indexed yet.
  fn index(
    db: &mut Connection,
    journal_root: &Path,
  ) -> Result<Self::Stats, Error>;

  /// How many records in its live files aren't indexed yet, counting up to
  /// `at_most`. Archives aren't counted: a newly archived day is mostly
  /// records already indexed from its live files.
  fn waiting(
    db: &Connection,
    journal_root: &Path,
    at_most: u64,
  ) -> Result<u64, Error>;
}
