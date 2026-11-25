//! dgg: its chat, captured into the journal, indexed, and filled in from
//! other archives where capture missed it.

pub mod backfill;
pub mod capture;
pub mod channels;
pub mod gaps;
pub mod index;
pub mod migrate;

use {
  crate::{
    index::Error,
    service::{Indexed, Root, Service},
  },
  ::common::fs::MissingOk,
  ::logger_journal::Variant,
  ::rusqlite::Connection,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// dgg, as a [`Service`]: a directory per channel in `dgg/`, `dgg/<name>/`
/// (see [`channels::valid_name`]), holding its journal and its backfill
/// journal (see [`backfill_root`](::logger_journal::layout::backfill_root)),
/// at `backfill/` in it.
pub struct Dgg;

impl Service for Dgg {
  const NAME: &'static str = "dgg";
  const API: &'static str = "v0";
  const VARIANT: Variant = Variant::Dgg;

  type Stats = index::Stats;

  fn dir(journal: &Path) -> PathBuf {
    journal.join(Self::NAME)
  }

  /// Every channel's, each channel's journal then its backfill journal.
  fn roots(dir: &Path) -> io::Result<Vec<Root>> {
    let mut roots = Vec::new();
    for name in Self::channels(dir)? {
      let channel = Self::channel_dir(dir, &name);
      for root in Self::channel_roots(&channel, &name) {
        roots.push(Root {
          prefix: format!("{name}/{}", root.prefix),
          ..root
        });
      }
    }
    Ok(roots)
  }

  fn valid_channel(name: &str) -> bool {
    channels::valid_name(name)
  }

  /// Every directory in `dir` named by a [`channels::valid_name`], whether
  /// or not the list still has it.
  fn channels(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    let Some(entries) = fs::read_dir(dir).missing_ok()? else {
      return Ok(names);
    };
    for entry in entries {
      let entry = entry?;
      let name = entry.file_name().to_string_lossy().into_owned();
      if entry.file_type()?.is_dir() && channels::valid_name(&name) {
        names.push(name);
      }
    }
    names.sort();
    Ok(names)
  }

  fn channel_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
  }

  fn channel_roots(dir: &Path, name: &str) -> Vec<Root> {
    let backfill = match name {
      "" => "backfill".to_owned(),
      name => format!("{name} backfill"),
    };
    vec![
      Root::new::<Self>(dir, "", name),
      Root::new::<Self>(dir, "backfill/", &backfill),
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
