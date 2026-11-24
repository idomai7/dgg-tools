//! Index: the journal into SQLite, for searching and for finding gaps.
//!
//! The index is derived data: it can always be deleted and rebuilt from the
//! journal. Each journal file's progress is checkpointed in the same
//! transaction as the rows it produced, so an interrupted run resumes
//! exactly where it stopped, with nothing indexed twice.
//!
//! Archives are found through the monthly manifests. An archive's
//! checkpoint also records its SHA-256: a fully read archive is skipped
//! without being opened, and one re-merged since (so its offsets no longer
//! line up) is read again from the start. A newly archived day is read once
//! more than its live files were, which deduplication makes harmless.
//!
//! Messages are deduplicated on `(timestamp, nick, data)`, verified to match
//! between live `MSG` frames and the `HISTORY` replay. A message seen both
//! ways is kept as `live`, with the earliest live receive time.
//!
//! Backfilled (`fetched`) messages have lowercased nicks and may have their
//! text altered (rustlesearch turns newlines into spaces), so they match on
//! timestamp and lowercased nick instead. A whole-second timestamp is taken
//! to be truncated (linkers.ooo, old rustle logs) and matches anything from
//! that user within its second. Native messages win: a backfilled one is
//! skipped if a native one is already there, and replaced when one arrives.
//! Likewise a backfilled message with a precise timestamp replaces a
//! truncated one. Its nick takes the capitalisation of the same user's
//! native messages, whenever those are indexed.

use {
  ::common::fs::MissingOk,
  ::logger_journal::{
    Entry, ReadError, Reader, Record, Version, archive,
    layout::{backfill_files, backfill_root, live_files},
    record::v0::Body,
  },
  ::rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    params,
  },
  ::serde_json::Value,
  ::std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
  },
};

/// Bumped whenever the schema changes; an index at another version must be
/// rebuilt. While the major is 0, every change bumps the minor.
pub const SCHEMA_VERSION: Version = Version { major: 0, minor: 6 };

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

/// The current schema. Every version's schema is kept in `schemas/index/`;
/// add a new file there (and bump [`SCHEMA_VERSION`]) rather than editing
/// an old one.
const SCHEMA: &str = include_str!("../schemas/index/v0.6.sql");

/// Records are committed in batches of this many, with their checkpoint.
const BATCH: usize = 10_000;

#[derive(Debug)]
pub enum Error {
  Io(io::Error),
  Sqlite(::rusqlite::Error),
  /// The index was built with another schema version; rebuild it.
  Schema(Version),
  /// Kick's index was, `index-kick.sqlite` (see [`crate::kick::index`]).
  KickSchema(Version),
}

impl Error {
  /// Whether no retry can fix it.
  pub fn fatal(&self) -> bool {
    matches!(self, Self::Schema(_) | Self::KickSchema(_))
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
      Self::Schema(version) => write!(
        f,
        "index has schema {}.{}, not {}.{}; run `index --rebuild`",
        version.major,
        version.minor,
        SCHEMA_VERSION.major,
        SCHEMA_VERSION.minor,
      ),
      Self::KickSchema(version) => {
        let wanted = crate::kick::index::SCHEMA_VERSION;
        write!(
          f,
          "Kick's index has schema {}.{}, not {}.{}; run `index \
           --rebuild-kick`",
          version.major, version.minor, wanted.major, wanted.minor,
        )
      }
    }
  }
}

/// What one [`index`] run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  /// Journal records read.
  pub records: u64,
  /// Messages added to the index (not counting ones already there).
  pub messages: u64,
  /// Of those, how many were backfilled.
  pub backfilled: u64,
  /// Records or HISTORY entries that couldn't be indexed; see `unparsed`.
  pub unparsed: u64,
  /// Files skipped because their header couldn't be read, with why.
  pub skipped: Vec<(String, String)>,
}

/// Opens the index at `path` only to read it, as it is: it must exist, with
/// this schema. Waits up to `busy` for a writer's lock.
pub fn open_read_only(
  path: &Path,
  busy: Duration,
) -> Result<Connection, Error> {
  open_read_only_at(path, busy, SCHEMA_VERSION, Error::Schema)
}

/// Opens the database at `path` only to read it: it must exist, at schema
/// `version`; one at another is `wrong`.
pub(crate) fn open_read_only_at(
  path: &Path,
  busy: Duration,
  version: Version,
  wrong: fn(Version) -> Error,
) -> Result<Connection, Error> {
  let db = Connection::open_with_flags(
    path,
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
  )?;
  db.busy_timeout(busy)?;
  let found: i64 =
    db.pragma_query_value(None, "user_version", |row| row.get(0))?;
  if found != user_version(version) {
    return Err(wrong(schema_version(found)));
  }
  Ok(db)
}

/// Opens the index at `path`, creating it if needed.
pub fn open(path: &Path) -> Result<Connection, Error> {
  open_at(path, SCHEMA, SCHEMA_VERSION, Error::Schema)
}

/// Opens the database at `path`, creating it with `schema`, at `version`,
/// if needed; one at another version is `wrong`.
pub(crate) fn open_at(
  path: &Path,
  schema: &str,
  version: Version,
  wrong: fn(Version) -> Error,
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
    other => return Err(wrong(schema_version(other))),
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

/// Deletes the index at `path` (and its WAL files), then opens a fresh one.
pub fn rebuild(path: &Path) -> Result<Connection, Error> {
  remove(path)?;
  open(path)
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

/// Keeps the index up to date while `running()` is true: runs [`index`] at
/// once, then as often as `pace` says, passing each run's stats to `report`.
///
/// Errors from a run are passed to `report` and retried next round (a file
/// briefly locked by another process, say), except [`fatal`](Error::fatal)
/// ones, which no retry can fix and so are returned.
pub fn follow(
  db: &mut Connection,
  journal_root: &Path,
  pace: Pace,
  running: impl Fn() -> bool,
  report: impl FnMut(Result<&Stats, &Error>),
) -> Result<(), Error> {
  paced(
    db,
    pace,
    running,
    |db, at_most| waiting(db, journal_root, at_most),
    |db| index(db, journal_root),
    report,
  )
}

/// Runs `round` on `db` at once, then as often as `pace` says while
/// `running()` is true, passing each one's stats to `report`; `waiting`
/// counts the records waiting, up to the count it's given. As [`follow`]
/// otherwise.
pub(crate) fn paced<S>(
  db: &mut Connection,
  pace: Pace,
  running: impl Fn() -> bool,
  waiting: impl Fn(&Connection, u64) -> Result<u64, Error>,
  round: impl Fn(&mut Connection) -> Result<S, Error>,
  mut report: impl FnMut(Result<&S, &Error>),
) -> Result<(), Error> {
  let mut last = None::<Instant>;
  while running() {
    let since = last.map(|last| last.elapsed());
    let due = match since {
      None => true,
      Some(since) if since >= pace.max_wait => true,
      Some(since) if since >= pace.min_wait => {
        // A failure to look is the round's to report.
        waiting(db, pace.batch).map_or(true, |n| n >= pace.batch)
      }
      Some(_) => false,
    };
    if due {
      last = Some(Instant::now());
      match round(db) {
        Ok(stats) => report(Ok(&stats)),
        Err(e) if e.fatal() => return Err(e),
        Err(e) => report(Err(&e)),
      }
    }
    ::std::thread::sleep(pace.look);
  }
  Ok(())
}

/// How many records in the live and backfill files aren't indexed yet,
/// counting up to `at_most`. Archives aren't counted: a newly archived day
/// is mostly records already indexed from its live files.
pub fn waiting(
  db: &Connection,
  journal_root: &Path,
  at_most: u64,
) -> Result<u64, Error> {
  let files = live_files(journal_root)?
    .into_iter()
    .chain(backfill_files(journal_root)?)
    .collect();
  waiting_in(db, journal_root, files, at_most)
}

/// How many records in `files`, live files under `journal_root`, aren't
/// indexed in `db` yet, counting up to `at_most`.
pub(crate) fn waiting_in(
  db: &Connection,
  journal_root: &Path,
  files: Vec<PathBuf>,
  at_most: u64,
) -> Result<u64, Error> {
  let mut count = 0;
  for file in files {
    if count >= at_most {
      break;
    }
    let (offset, _) = checkpoint(db, &relative(journal_root, &file))?;
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

/// Indexes everything in the journal at `journal_root` that isn't indexed
/// yet.
pub fn index(db: &mut Connection, journal_root: &Path) -> Result<Stats, Error> {
  let mut stats = Stats::default();
  let relative = |file: &Path| relative(journal_root, file);
  let mut live = Vec::new();
  let files = live_files(journal_root)?
    .into_iter()
    .chain(backfill_files(journal_root)?);
  for file in files {
    let path = relative(&file);
    let (offset, _) = checkpoint(db, &path)?;
    live.push(path.clone());
    let reader = match Reader::open(&file, offset) {
      Ok(reader) => reader,
      Err(ReadError::Io(e)) => return Err(e.into()),
      Err(e) => {
        stats.skipped.push((path, format!("{e:?}")));
        continue;
      }
    };
    index_file(db, &path, None, reader, &mut stats)?;
  }
  let backfill = backfill_root(journal_root);
  let archives = archive::all(journal_root)?
    .into_iter()
    .map(|archive| (journal_root, archive))
    .chain(
      archive::all(&backfill)?
        .into_iter()
        .map(|archive| (backfill.as_path(), archive)),
    );
  for (root, archive) in archives {
    let file = archive.path(root);
    let path = relative(&file);
    let (offset, digest) = checkpoint(db, &path)?;
    let same = digest.as_deref() == Some(archive.sha256.as_str());
    if same && offset >= archive.bytes {
      continue;
    }
    let offset = if same { offset } else { 0 };
    let reader = match Reader::open(&file, offset) {
      Ok(reader) => reader,
      Err(ReadError::Io(e)) if e.kind() != io::ErrorKind::NotFound => {
        return Err(e.into());
      }
      Err(e) => {
        stats.skipped.push((path, format!("{e:?}")));
        continue;
      }
    };
    index_file(db, &path, Some(&archive.sha256), reader, &mut stats)?;
  }
  forget_gone(db, &live)?;
  Ok(stats)
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

/// Drops what the index holds of live and backfill files that no longer
/// exist (compacted away): their checkpoints and their unparsed rows.
fn forget_gone(db: &mut Connection, live: &[String]) -> Result<(), Error> {
  let transaction = db.transaction()?;
  // Unparsed rows too, even ones a build from before this left behind.
  let stale: Vec<String> = transaction
    .prepare(
      "SELECT path FROM files
       WHERE path LIKE 'live/%' OR path LIKE 'backfill/live/%'
       UNION
       SELECT path FROM unparsed
       WHERE path LIKE 'live/%' OR path LIKE 'backfill/live/%'",
    )?
    .query_map([], |row| row.get(0))?
    .collect::<Result<Vec<String>, _>>()?
    .into_iter()
    .filter(|path| !live.contains(path))
    .collect();
  for path in stale {
    transaction.execute("DELETE FROM files WHERE path = ?1", [&path])?;
    transaction.execute("DELETE FROM unparsed WHERE path = ?1", [&path])?;
  }
  transaction.commit()?;
  Ok(())
}

fn index_file(
  db: &mut Connection,
  path: &str,
  digest: Option<&str>,
  mut reader: Reader,
  stats: &mut Stats,
) -> Result<(), Error> {
  loop {
    let transaction = db.transaction()?;
    let mut records = 0;
    // Latest receive time per connection in this batch.
    let mut last_seen = HashMap::new();
    for entry in reader.by_ref().take(BATCH) {
      records += 1;
      match entry {
        Ok(Entry {
          offset,
          record: Record::Kick0(_),
          ..
        }) => {
          unparsed(&transaction, path, offset, 0, "a Kick record, in dgg's")?;
          stats.unparsed += 1;
        }
        Ok(Entry {
          offset,
          record: Record::V0(record),
          ..
        }) => {
          let key = (record.instance.clone(), record.connection);
          let seen = last_seen.entry(key).or_insert(record.receive_time);
          *seen = record.receive_time.max(*seen);
          let origin = Origin {
            path,
            offset,
            instance: &record.instance,
            connection: record.connection,
            received: record.receive_time,
          };
          index_body(&transaction, &origin, record.body, stats)?;
        }
        Err(ReadError::Record { offset, error }) => {
          unparsed(&transaction, path, offset, 0, &format!("{error:?}"))?;
          stats.unparsed += 1;
        }
        Err(ReadError::Io(e)) => return Err(e.into()),
        Err(e) => return Err(io::Error::other(format!("{e:?}")).into()),
      }
    }
    for ((instance, connection), seen) in last_seen {
      transaction.execute(
        "UPDATE connections SET last_seen = max(last_seen, ?3)
          WHERE instance = ?1 AND connection = ?2",
        params![instance, connection as i64, seen],
      )?;
    }
    transaction.execute(
      "INSERT INTO files (path, offset, digest) VALUES (?1, ?2, ?3)
        ON CONFLICT (path) DO UPDATE SET
          offset = excluded.offset, digest = excluded.digest",
      params![path, reader.offset() as i64, digest],
    )?;
    transaction.commit()?;
    stats.records += records;
    if records < BATCH as u64 {
      return Ok(());
    }
  }
}

/// Where a record came from.
struct Origin<'a> {
  path: &'a str,
  offset: u64,
  instance: &'a str,
  connection: u64,
  received: i64,
}

fn index_body(
  db: &Transaction,
  origin: &Origin,
  body: Body,
  stats: &mut Stats,
) -> Result<(), Error> {
  match body {
    Body::Open(url) => {
      db.execute(
        "INSERT OR IGNORE INTO connections
          (instance, connection, url, opened, last_seen)
          VALUES (?1, ?2, ?3, ?4, ?4)",
        params![
          origin.instance,
          origin.connection as i64,
          url,
          origin.received
        ],
      )?;
    }
    Body::Close(reason) => {
      db.execute(
        "UPDATE connections SET closed = ?3, close_reason = ?4
          WHERE instance = ?1 AND connection = ?2",
        params![
          origin.instance,
          origin.connection as i64,
          origin.received,
          reason
        ],
      )?;
    }
    Body::Frame(frame) => {
      let (event, payload) = frame.split_once(' ').unwrap_or((&frame, ""));
      match event {
        "MSG" => {
          message(db, origin, 0, payload, "live", stats)?;
        }
        "HISTORY" => history(db, origin, payload, stats)?,
        _ => {}
      }
    }
    Body::Fetched {
      source,
      timestamp,
      nick,
      text,
    } => fetched(db, origin, &source, timestamp, &nick, &text, stats)?,
    Body::FetchRange { source, from, to } => {
      db.execute(
        "INSERT OR IGNORE INTO fetch_ranges (source, start_time, end_time)
          VALUES (?1, ?2, ?3)",
        params![source, from, to],
      )?;
    }
    Body::Binary(_) => {}
  }
  Ok(())
}

/// Indexes each `MSG` in a HISTORY frame's JSON array of frames, and notes
/// the oldest one on the connection so gaps it covers can be told apart.
fn history(
  db: &Transaction,
  origin: &Origin,
  payload: &str,
  stats: &mut Stats,
) -> Result<(), Error> {
  let Ok(Value::Array(entries)) = ::serde_json::from_str(payload) else {
    unparsed(
      db,
      origin.path,
      origin.offset,
      0,
      "HISTORY not a JSON array",
    )?;
    stats.unparsed += 1;
    return Ok(());
  };
  let mut oldest = None;
  for (item, entry) in (1..).zip(entries) {
    let Value::String(frame) = entry else {
      unparsed(
        db,
        origin.path,
        origin.offset,
        item,
        "HISTORY entry not a frame",
      )?;
      stats.unparsed += 1;
      continue;
    };
    if let Some(payload) = frame.strip_prefix("MSG ")
      && let Some(timestamp) =
        message(db, origin, item, payload, "history", stats)?
    {
      oldest = Some(oldest.map_or(timestamp, |o: i64| o.min(timestamp)));
    }
  }
  if let Some(oldest) = oldest {
    db.execute(
      "UPDATE connections SET history_from = min(coalesce(history_from, ?3), ?3)
        WHERE instance = ?1 AND connection = ?2",
      params![origin.instance, origin.connection as i64, oldest],
    )?;
  }
  Ok(())
}

/// Indexes one `MSG` payload, deduplicating against what's already there.
/// Returns its timestamp, or `None` if it couldn't be parsed.
fn message(
  db: &Transaction,
  origin: &Origin,
  item: u64,
  payload: &str,
  source: &str,
  stats: &mut Stats,
) -> Result<Option<i64>, Error> {
  let parsed = ::serde_json::from_str::<Value>(payload)
    .map_err(|_| "MSG not JSON")
    .and_then(|value| {
      let text = |key| value[key].as_str().ok_or(key);
      Ok((
        value["timestamp"].as_i64().ok_or("MSG without timestamp")?,
        text("nick").map_err(|_| "MSG without nick")?.to_owned(),
        text("data").map_err(|_| "MSG without data")?.to_owned(),
        value.get("features").map_or("[]".into(), Value::to_string),
      ))
    });
  let (timestamp, nick, data, features) = match parsed {
    Ok(parsed) => parsed,
    Err(reason) => {
      unparsed(db, origin.path, origin.offset, item, reason)?;
      stats.unparsed += 1;
      return Ok(None);
    }
  };
  remove_backfilled(db, timestamp, &nick)?;
  let inserted = insert(
    db,
    timestamp,
    &nick,
    &data,
    &features,
    origin.received,
    source,
  )?;
  if inserted {
    stats.messages += 1;
    // Re-case any backfilled messages from this user.
    db.execute(
      "UPDATE messages SET nick = ?1
        WHERE lower(nick) = lower(?1) AND nick <> ?1
          AND source LIKE 'backfill:%'",
      [&nick],
    )?;
  } else if source == "live" {
    // Already there: a live sighting replaces a HISTORY one, and the
    // earliest live receive time wins.
    db.execute(
      "UPDATE messages SET
          received = CASE WHEN source = 'live'
            THEN min(received, ?4) ELSE ?4 END,
          source = 'live'
        WHERE timestamp = ?1 AND nick = ?2 AND data = ?3",
      params![timestamp, nick, data, origin.received],
    )?;
  }
  Ok(Some(timestamp))
}

/// Indexes one backfilled message, unless a message from the same user at
/// the same time is already there. A whole-second timestamp is taken to be
/// truncated, so it matches anything from that user within its second.
fn fetched(
  db: &Transaction,
  origin: &Origin,
  source: &str,
  timestamp: i64,
  nick: &str,
  text: &str,
  stats: &mut Stats,
) -> Result<(), Error> {
  // Native casing first; failing that, a source that keeps case. This
  // happens even for a message that's skipped, so the result doesn't
  // depend on the order sources are indexed in.
  let mut cased: Option<String> = db
    .query_row(
      "SELECT nick FROM messages
        WHERE lower(nick) = lower(?1) AND source NOT LIKE 'backfill:%'
        LIMIT 1",
      [nick],
      |row| row.get(0),
    )
    .optional()?;
  if cased.is_none() && nick == nick.to_lowercase() {
    cased = db
      .query_row(
        "SELECT nick FROM messages
          WHERE lower(nick) = lower(?1) AND nick <> lower(nick)
          LIMIT 1",
        [nick],
        |row| row.get(0),
      )
      .optional()?;
  } else if cased.is_none() {
    // Re-case this user's lowercased backfilled messages.
    db.execute(
      "UPDATE messages SET nick = ?1
        WHERE lower(nick) = lower(?1) AND nick = lower(nick)
          AND source LIKE 'backfill:%'",
      [nick],
    )?;
  }
  let end = if whole_second(timestamp) {
    timestamp + 1000
  } else {
    timestamp + 1
  };
  let exists: bool = db.query_row(
    "SELECT EXISTS (SELECT 1 FROM messages
      WHERE timestamp >= ?1 AND timestamp < ?2 AND lower(nick) = lower(?3))",
    params![timestamp, end, nick],
    |row| row.get(0),
  )?;
  if exists {
    return Ok(());
  }
  // A precise timestamp supersedes a truncated one.
  remove_backfilled(db, timestamp, nick)?;
  let nick = cased.as_deref().unwrap_or(nick);
  let source = format!("backfill:{source}");
  if insert(db, timestamp, nick, text, "[]", origin.received, &source)? {
    stats.messages += 1;
    stats.backfilled += 1;
  }
  Ok(())
}

/// Inserts a message and its full-text entry, unless it's already there.
/// Returns whether it was inserted.
fn insert(
  db: &Transaction,
  timestamp: i64,
  nick: &str,
  data: &str,
  features: &str,
  received: i64,
  source: &str,
) -> Result<bool, Error> {
  let inserted = db.execute(
    "INSERT OR IGNORE INTO messages
      (timestamp, nick, data, features, received, source)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    params![timestamp, nick, data, features, received, source],
  )?;
  if inserted == 1 {
    db.execute(
      "INSERT INTO messages_fts (rowid, data) VALUES (?1, ?2)",
      params![db.last_insert_rowid(), data],
    )?;
  }
  Ok(inserted == 1)
}

/// Whether `timestamp` (unix ms) falls on a whole second, as truncated
/// timestamps from some sources do.
fn whole_second(timestamp: i64) -> bool {
  timestamp.rem_euclid(1000) == 0
}

/// Removes backfilled messages that a message at `timestamp` from `nick`
/// supersedes: those at exactly that time, and those truncated to its
/// second. Their full-text entries go too.
fn remove_backfilled(
  db: &Transaction,
  timestamp: i64,
  nick: &str,
) -> Result<(), Error> {
  let mut query = db.prepare_cached(
    "SELECT id, data FROM messages
      WHERE timestamp IN (?1, ?2) AND lower(nick) = lower(?3)
        AND source LIKE 'backfill:%'",
  )?;
  let second = timestamp - timestamp.rem_euclid(1000);
  let superseded: Vec<(i64, String)> = query
    .query_map(params![timestamp, second, nick], |row| {
      Ok((row.get(0)?, row.get(1)?))
    })?
    .collect::<Result<_, _>>()?;
  for (id, data) in superseded {
    db.execute(
      "INSERT INTO messages_fts (messages_fts, rowid, data)
        VALUES ('delete', ?1, ?2)",
      params![id, data],
    )?;
    db.execute("DELETE FROM messages WHERE id = ?1", [id])?;
  }
  Ok(())
}

fn unparsed(
  db: &Transaction,
  path: &str,
  offset: u64,
  item: u64,
  reason: &str,
) -> Result<(), Error> {
  db.execute(
    "INSERT OR REPLACE INTO unparsed (path, offset, item, reason)
      VALUES (?1, ?2, ?3, ?4)",
    params![path, offset as i64, item as i64, reason],
  )?;
  Ok(())
}
