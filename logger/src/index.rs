//! What every service's index shares: opening it at its schema, its files'
//! checkpoints, and keeping it up to date at a pace. Each service's own is
//! in its module: [`dgg::index`](crate::dgg::index) and
//! [`kick::index`](crate::kick::index).
//!
//! An index is derived data: it can always be deleted and rebuilt from the
//! journal. Each journal file's progress is checkpointed in the same
//! transaction as the rows it produced, so an interrupted run resumes
//! exactly where it stopped, with nothing indexed twice.

use {
  crate::service::Service,
  ::common::fs::MissingOk,
  ::logger_journal::{ReadError, Reader, Version},
  ::rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior},
  ::std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
  },
};

/// How a schema version is stored in SQLite's `user_version`:
/// `major << 16 | minor`.
pub fn user_version(version: Version) -> i64 {
  i64::from(version.major) << 16 | i64::from(version.minor)
}

/// The schema version a `user_version` stands for.
pub fn schema_version(user_version: i64) -> Version {
  Version {
    major: (user_version >> 16) as u32,
    minor: (user_version & 0xffff) as u32,
  }
}

#[derive(Debug)]
pub enum Error {
  Io(io::Error),
  Sqlite(::rusqlite::Error),
  /// A service's index was built with another schema version; rebuild it.
  Schema {
    /// The service's [name](Service::NAME).
    service: &'static str,
    found: Version,
    wanted: Version,
  },
}

impl Error {
  /// Whether no retry can fix it.
  pub fn fatal(&self) -> bool {
    matches!(self, Self::Schema { .. })
  }
}

impl From<io::Error> for Error {
  fn from(e: io::Error) -> Self {
    Self::Io(e)
  }
}

impl From<::rusqlite::Error> for Error {
  fn from(e: ::rusqlite::Error) -> Self {
    Self::Sqlite(e)
  }
}

impl ::std::fmt::Display for Error {
  fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
    match self {
      Self::Io(e) => write!(f, "{e}"),
      Self::Sqlite(e) => write!(f, "sqlite: {e}"),
      Self::Schema {
        service,
        found,
        wanted,
      } => write!(
        f,
        "the {service} index has schema {}.{}, not {}.{}; run `index \
         --rebuild-{service}`",
        found.major, found.minor, wanted.major, wanted.minor,
      ),
    }
  }
}

/// What an index round's stats have in common, for reporting them.
pub trait Tally: Default {
  /// Journal records read.
  fn records(&self) -> u64;
  /// Files skipped because their header couldn't be read, with why.
  fn skipped(&self) -> &[(String, String)];
  /// Live files gone from the journal since, whose checkpoints and
  /// unparsed rows were dropped.
  fn forgotten(&self) -> &[String];
  /// Adds `other`'s counts to these.
  fn add(&mut self, other: &Self);
  /// The counts, as a line.
  fn summary(&self) -> String;
}

/// Opens the database at `path` only to read it: it must exist, at
/// `service`'s schema `version`. Waits up to `busy` for a writer's lock.
pub(crate) fn open_read_only_at(
  path: &Path,
  busy: Duration,
  service: &'static str,
  version: Version,
) -> Result<Connection, Error> {
  let db = Connection::open_with_flags(
    path,
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
  )?;
  db.busy_timeout(busy)?;
  let found: i64 =
    db.pragma_query_value(None, "user_version", |row| row.get(0))?;
  if found != user_version(version) {
    return Err(Error::Schema {
      service,
      found: schema_version(found),
      wanted: version,
    });
  }
  Ok(db)
}

/// Opens the database at `path`, creating it with `schema`, `service`'s at
/// `version`, if needed.
pub(crate) fn open_at(
  path: &Path,
  service: &'static str,
  schema: &str,
  version: Version,
) -> Result<Connection, Error> {
  let mut db = Connection::open(path)?;
  // Other processes (a follower, a report) may hold the lock briefly; wait
  // for it rather than failing at once.
  db.busy_timeout(Duration::from_secs(10))?;
  set_wal(&db)?;
  db.pragma_update(None, "synchronous", "NORMAL")?;
  // Check and create the schema holding the write lock, so two processes
  // opening a new index at once can't both try to create it.
  let transaction =
    db.transaction_with_behavior(TransactionBehavior::Immediate)?;
  let found: i64 =
    transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
  match found {
    0 => {
      transaction.execute_batch(schema)?;
      transaction.pragma_update(None, "user_version", user_version(version))?;
    }
    found if found == user_version(version) => {}
    other => {
      return Err(Error::Schema {
        service,
        found: schema_version(other),
        wanted: version,
      });
    }
  }
  transaction.commit()?;
  Ok(db)
}

/// Switches the database to WAL mode if it isn't already. Switching needs
/// an exclusive lock, and SQLite reports busy at once rather than waiting
/// for one, so a fresh database opened by two processes at once is retried
/// for a while.
fn set_wal(db: &Connection) -> Result<(), Error> {
  let deadline = ::std::time::Instant::now() + Duration::from_secs(10);
  loop {
    let mode: String =
      db.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
      return Ok(());
    }
    match db.pragma_update(None, "journal_mode", "WAL") {
      Err(::rusqlite::Error::SqliteFailure(e, _))
        if e.code == ::rusqlite::ErrorCode::DatabaseBusy
          && ::std::time::Instant::now() < deadline =>
      {
        ::std::thread::sleep(Duration::from_millis(10));
      }
      result => result?,
    }
  }
}

/// Deletes `S`'s index at `path` (and its WAL files), then opens a fresh
/// one.
pub fn rebuild<S: Service>(path: &Path) -> Result<Connection, Error> {
  remove(path)?;
  S::open_index(path)
}

/// Deletes the database at `path`, and its WAL files.
pub(crate) fn remove(path: &Path) -> io::Result<()> {
  for suffix in ["", "-wal", "-shm"] {
    let mut file = path.as_os_str().to_owned();
    file.push(suffix);
    fs::remove_file(file).missing_ok()?;
  }
  Ok(())
}

/// How often [`follow`] indexes: a round once `batch` records are waiting,
/// but no sooner than `min_wait` after the last, and at the latest
/// `max_wait` after it however few are waiting.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
  /// How often to look at what's waiting.
  pub look: Duration,
  pub batch: u64,
  pub min_wait: Duration,
  pub max_wait: Duration,
}

/// Keeps `S`'s index of the journals in `dir` (see
/// [`Indexed::dir`](crate::service::Indexed::dir)) up to date while
/// `running()` is true: indexes at once, then as often as `pace` says,
/// passing each round's stats to `report`.
///
/// Errors from a round are passed to `report` and retried next round (a
/// file briefly locked by another process, say), except
/// [`fatal`](Error::fatal) ones, which no retry can fix and so are
/// returned.
pub fn follow<S: Service>(
  db: &mut Connection,
  dir: &Path,
  pace: Pace,
  running: impl Fn() -> bool,
  mut report: impl FnMut(Result<&S::Stats, &Error>),
) -> Result<(), Error> {
  let mut last = None::<Instant>;
  while running() {
    let since = last.map(|last| last.elapsed());
    let due = match since {
      None => true,
      Some(since) if since >= pace.max_wait => true,
      Some(since) if since >= pace.min_wait => {
        // A failure to look is the round's to report.
        S::waiting(db, dir, pace.batch).map_or(true, |n| n >= pace.batch)
      }
      Some(_) => false,
    };
    if due {
      last = Some(Instant::now());
      match S::index(db, dir) {
        Ok(stats) => report(Ok(&stats)),
        Err(e) if e.fatal() => return Err(e),
        Err(e) => report(Err(&e)),
      }
    }
    ::std::thread::sleep(pace.look);
  }
  Ok(())
}

/// How many records in `files`, live files each with its path as the index
/// names it, aren't indexed in `db` yet, counting up to `at_most`.
pub(crate) fn waiting_in(
  db: &Connection,
  files: Vec<(String, PathBuf)>,
  at_most: u64,
) -> Result<u64, Error> {
  let mut count = 0;
  for (path, file) in files {
    if count >= at_most {
      break;
    }
    let (offset, _) = checkpoint(db, &path)?;
    // Only a file that's grown since is opened.
    if fs::metadata(&file)
      .missing_ok()?
      .is_none_or(|m| m.len() <= offset)
    {
      continue;
    }
    count += match Reader::open(&file, offset) {
      Ok(reader) => reader.count_records(at_most - count)?,
      Err(ReadError::Io(e)) if e.kind() != io::ErrorKind::NotFound => {
        return Err(e.into());
      }
      // Gone, or one a round would skip.
      Err(_) => 0,
    };
  }
  Ok(count)
}

/// `file`'s path under `journal_root`, as the index names it.
pub(crate) fn relative(journal_root: &Path, file: &Path) -> String {
  let relative = file.strip_prefix(journal_root).unwrap_or(file);
  relative.to_string_lossy().replace('\\', "/")
}

/// A file's checkpoint: where to resume, and the digest it was read at.
pub(crate) fn checkpoint(
  db: &Connection,
  path: &str,
) -> Result<(u64, Option<String>), Error> {
  let found = db
    .query_row(
      "SELECT offset, digest FROM files WHERE path = ?1",
      [path],
      |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)),
    )
    .optional()?;
  Ok(found.unwrap_or((0, None)))
}

/// Drops what `db` holds of live files, those whose paths start with one
/// of `live_prefixes`, that aren't in `live` any more (compacted away, or
/// lost): their checkpoints and their unparsed rows, even ones a build
/// from before this left behind. Returns their paths.
pub(crate) fn forget_gone(
  db: &mut Connection,
  live_prefixes: &[&str],
  live: &[String],
) -> Result<Vec<String>, Error> {
  let transaction = db.transaction()?;
  let gone: Vec<String> = transaction
    .prepare("SELECT path FROM files UNION SELECT path FROM unparsed")?
    .query_map([], |row| row.get(0))?
    .collect::<Result<Vec<String>, _>>()?
    .into_iter()
    .filter(|path: &String| {
      live_prefixes.iter().any(|prefix| path.starts_with(prefix))
        && !live.contains(path)
    })
    .collect();
  for path in &gone {
    transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
    transaction.execute("DELETE FROM unparsed WHERE path = ?1", [path])?;
  }
  transaction.commit()?;
  Ok(gone)
}
