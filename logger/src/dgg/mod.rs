//! dgg: its chat, captured into the journal, indexed, and filled in from
//! other archives where capture missed it.

pub mod backfill;
pub mod capture;
pub mod channels;
pub mod gaps;
pub mod index;

use {
  crate::{
    index::Error,
    service::{Indexed, Root, Service},
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
/// `backfill/` in it. They're one channel's, [`channels::DESTINY`]'s, and
/// `dgg/` is its directory; any other channel's would be in `dgg/<name>/`.
pub struct Dgg;

impl Service for Dgg {
  const NAME: &'static str = "dgg";
  const API: &'static str = "v0";
  const VARIANT: Variant = Variant::Dgg;

  type Stats = index::Stats;

  fn dir(journal: &Path) -> PathBuf {
    journal.join(Self::NAME)
  }

  fn roots(dir: &Path) -> io::Result<Vec<Root>> {
    Ok(Self::channel_roots(dir, channels::DESTINY))
  }

  fn valid_channel(name: &str) -> bool {
    channels::valid_name(name)
  }

  fn channels(_: &Path) -> io::Result<Vec<String>> {
    Ok(vec![channels::DESTINY.into()])
  }

  fn channel_dir(dir: &Path, name: &str) -> PathBuf {
    match name {
      channels::DESTINY => dir.to_owned(),
      name => dir.join(name),
    }
  }

  fn channel_roots(dir: &Path, _: &str) -> Vec<Root> {
    vec![
      Root::new::<Self>(dir, "", ""),
      Root::new::<Self>(dir, "backfill/", "backfill"),
    ]
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

  /// One per channel, `index-dgg-<name>.sqlite`.
  fn indexes(dir: &Path) -> io::Result<Vec<Indexed>> {
    let channels = Self::channels(dir)?;
    Ok(
      channels
        .iter()
        .map(|name| Indexed {
          name: format!("{} {name}", Self::NAME),
          file: index::file(name),
          dir: Self::channel_dir(dir, name),
        })
        .collect(),
    )
  }

  fn index(db: &mut Connection, dir: &Path) -> Result<index::Stats, Error> {
    index::index(db, dir)
  }

  fn waiting(db: &Connection, dir: &Path, at_most: u64) -> Result<u64, Error> {
    index::waiting(db, dir, at_most)
  }
}
