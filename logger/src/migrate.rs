//! Migrate: brings the journal and the index up to this build's formats, in
//! place, as a supervisor starts.
//!
//! There's nothing to migrate: every host is on this build's formats
//! (`journal dgg.0.3`, index schema 0.6), so it only checks the index is
//! at [`SCHEMA_VERSION`], or that there's none yet. The next format change
//! that needs one fills it in.
//!
//! It holds `index.lock`, `compact.lock` and `sync.lock` in the data
//! directory while it runs, waiting for them as a supervisor does, so
//! nothing indexes, compacts or syncs meanwhile. A dry run changes nothing,
//! and takes no locks.

use {
  crate::index::{SCHEMA_VERSION, schema_version},
  ::logger_journal::Version,
  ::rusqlite::{Connection, OpenFlags},
  ::std::{
    fs::{File, TryLockError},
    io,
    path::Path,
    thread,
    time::Duration,
  },
};

/// The locks in the data directory it holds, a supervisor's.
pub const LOCKS: [&str; 3] = ["index.lock", "compact.lock", "sync.lock"];

/// What a [`migrate`] did, or with a dry run would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
  /// What was left as it was, with why.
  pub failed: Vec<String>,
  /// The index's schema, if there's an index.
  pub index: Option<Version>,
}

/// Migrates `<data>/journal` and `<data>/index.sqlite` (see the [module
/// docs](self)), saying what it does to `say`. Fails only if it can't go on
/// at all; what it can't migrate is in the report.
pub fn migrate(
  data: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(&str),
) -> io::Result<Report> {
  let _locks = if dry_run {
    Vec::new()
  } else {
    hold(data, say)?
  };
  let mut report = Report::default();
  let schema = |v: Version| format!("{}.{}", v.major, v.minor);
  match index_schema(&data.join("index.sqlite"))? {
    None => say("index: none yet"),
    Some(version) => {
      report.index = Some(version);
      if version == SCHEMA_VERSION {
        say(&format!(
          "index: schema {}, as it should be",
          schema(version)
        ));
      } else {
        let why = format!(
          "schema {}, not {}; rebuild it",
          schema(version),
          schema(SCHEMA_VERSION)
        );
        say(&format!("index: {why}"));
        report.failed.push(format!("index: {why}"));
      }
    }
  }
  Ok(report)
}

/// Waits for each of [`LOCKS`] in `data`, and holds them.
fn hold(data: &Path, say: &mut dyn FnMut(&str)) -> io::Result<Vec<File>> {
  let mut held = Vec::new();
  for name in LOCKS {
    let path = data.join(name);
    let mut said = false;
    loop {
      let file = File::create(&path)?;
      match file.try_lock() {
        Ok(()) => {
          held.push(file);
          break;
        }
        Err(TryLockError::WouldBlock) => {
          if !said {
            say(&format!("waiting for {name}: another supervisor has it"));
            said = true;
          }
          thread::sleep(Duration::from_secs(1));
        }
        Err(TryLockError::Error(e)) => return Err(e),
      }
    }
  }
  Ok(held)
}

/// The schema of the index at `path`, if there is one.
fn index_schema(path: &Path) -> io::Result<Option<Version>> {
  if !path.exists() {
    return Ok(None);
  }
  let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    .map_err(io::Error::other)?;
  db.busy_timeout(Duration::from_secs(10))
    .map_err(io::Error::other)?;
  let version: i64 = db
    .pragma_query_value(None, "user_version", |row| row.get(0))
    .map_err(io::Error::other)?;
  Ok(Some(schema_version(version)))
}
