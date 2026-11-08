//! Index: the journal into SQLite, for searching and for finding gaps.
//!
//! The index is derived data: it can always be deleted and rebuilt from the
//! journal. Each journal file's progress is checkpointed in the same
//! transaction as the rows it produced, so an interrupted run resumes
//! exactly where it stopped, with nothing indexed twice.
//!
//! Messages are deduplicated on `(timestamp, nick, data)`, verified to match
//! between live `MSG` frames and the `HISTORY` replay. A message seen both
//! ways is kept as `live`, with the earliest live receive time.
//!
//! Backfilled (`fetched`) messages have lowercased nicks and may have their
//! text altered (rustlesearch turns newlines into spaces), so they match on
//! timestamp and lowercased nick instead. Native messages win: a backfilled
//! one is skipped if a native one is already there, and replaced when one
//! arrives. Its nick takes the capitalisation of the same user's native
//! messages, whenever those are indexed.

use {
  ::journal::{
    ReadError, Reader, Record, layout::live_files, record::v1::Body,
  },
  ::rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params,
  },
  ::serde_json::Value,
  ::std::{collections::HashMap, fs, io, path::Path, time::Duration},
};

/// Bumped whenever the schema changes; an index at another version must be
/// rebuilt.
pub const SCHEMA_VERSION: i64 = 3;

/// The current schema. Every version's schema is kept in `schemas/index/`;
/// add a new file there (and bump [`SCHEMA_VERSION`]) rather than editing
/// an old one.
const SCHEMA: &str = include_str!("../schemas/index/v3.sql");

/// Lines are committed in batches of this many, with their checkpoint.
const BATCH: usize = 10_000;

#[derive(Debug)]
pub enum Error {
  Io(io::Error),
  Sqlite(::rusqlite::Error),
  /// The index was built with another schema version; rebuild it.
  Schema(i64),
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
        "index has schema {version}, not {SCHEMA_VERSION}; \
         run `index --rebuild`"
      ),
    }
  }
}

/// What one [`index`] run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  /// Journal lines read.
  pub lines: u64,
  /// Messages added to the index (not counting ones already there).
  pub messages: u64,
  /// Of those, how many were backfilled.
  pub backfilled: u64,
  /// Lines or HISTORY entries that couldn't be indexed; see `unparsed`.
  pub unparsed: u64,
  /// Files skipped because their header couldn't be read, with why.
  pub skipped: Vec<(String, String)>,
}

/// Opens the index at `path`, creating it if needed.
pub fn open(path: &Path) -> Result<Connection, Error> {
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
  let version: i64 =
    transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
  match version {
    0 => {
      transaction.execute_batch(SCHEMA)?;
      transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    SCHEMA_VERSION => {}
    other => return Err(Error::Schema(other)),
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
  for suffix in ["", "-wal", "-shm"] {
    let mut file = path.as_os_str().to_owned();
    file.push(suffix);
    match fs::remove_file(file) {
      Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
      _ => {}
    }
  }
  open(path)
}

/// Keeps the index up to date: runs [`index`] every `interval` while
/// `running()` is true, passing each run's stats to `report`.
///
/// Errors from a run are passed to `report` and retried next round (a file
/// briefly locked by another process, say), except [`Error::Schema`], which
/// no retry can fix and so is returned.
pub fn follow(
  db: &mut Connection,
  journal_root: &Path,
  interval: Duration,
  running: impl Fn() -> bool,
  mut report: impl FnMut(Result<&Stats, &Error>),
) -> Result<(), Error> {
  while running() {
    match index(db, journal_root) {
      Ok(stats) => report(Ok(&stats)),
      Err(e @ Error::Schema(_)) => return Err(e),
      Err(e) => report(Err(&e)),
    }
    ::std::thread::sleep(interval);
  }
  Ok(())
}

/// Indexes everything in the journal at `journal_root` that isn't indexed
/// yet.
pub fn index(db: &mut Connection, journal_root: &Path) -> Result<Stats, Error> {
  let mut stats = Stats::default();
  for file in live_files(journal_root)? {
    let relative = file.strip_prefix(journal_root).unwrap_or(&file);
    let relative = relative.to_string_lossy().replace('\\', "/");
    let offset: i64 = db
      .query_row(
        "SELECT offset FROM files WHERE path = ?",
        [&relative],
        |row| row.get(0),
      )
      .optional()?
      .unwrap_or(0);
    let reader = match Reader::open(&file, offset as u64) {
      Ok(reader) => reader,
      Err(ReadError::Io(e)) => return Err(e.into()),
      Err(e) => {
        stats.skipped.push((relative, format!("{e:?}")));
        continue;
      }
    };
    index_file(db, &relative, reader, &mut stats)?;
  }
  Ok(stats)
}

fn index_file(
  db: &mut Connection,
  path: &str,
  mut reader: Reader,
  stats: &mut Stats,
) -> Result<(), Error> {
  loop {
    let transaction = db.transaction()?;
    let mut lines = 0;
    // Latest receive time per connection in this batch.
    let mut last_seen = HashMap::new();
    for entry in reader.by_ref().take(BATCH) {
      lines += 1;
      match entry {
        Ok(entry) => {
          let Record::V1(record) = entry.record;
          let key = (record.instance.clone(), record.connection);
          let seen = last_seen.entry(key).or_insert(record.receive_time);
          *seen = record.receive_time.max(*seen);
          let line = Line {
            path,
            offset: entry.offset,
            instance: &record.instance,
            connection: record.connection,
            received: record.receive_time,
          };
          index_body(&transaction, &line, record.body, stats)?;
        }
        Err(ReadError::Line { offset, error }) => {
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
      "INSERT INTO files (path, offset) VALUES (?1, ?2)
        ON CONFLICT (path) DO UPDATE SET offset = excluded.offset",
      params![path, reader.offset() as i64],
    )?;
    transaction.commit()?;
    stats.lines += lines;
    if lines < BATCH as u64 {
      return Ok(());
    }
  }
}

/// Where a record came from.
struct Line<'a> {
  path: &'a str,
  offset: u64,
  instance: &'a str,
  connection: u64,
  received: i64,
}

fn index_body(
  db: &Transaction,
  line: &Line,
  body: Body,
  stats: &mut Stats,
) -> Result<(), Error> {
  match body {
    Body::Open(url) => {
      db.execute(
        "INSERT OR IGNORE INTO connections
          (instance, connection, url, opened, last_seen)
          VALUES (?1, ?2, ?3, ?4, ?4)",
        params![line.instance, line.connection as i64, url, line.received],
      )?;
    }
    Body::Close(reason) => {
      db.execute(
        "UPDATE connections SET closed = ?3, close_reason = ?4
          WHERE instance = ?1 AND connection = ?2",
        params![line.instance, line.connection as i64, line.received, reason],
      )?;
    }
    Body::Frame(frame) => {
      let (event, payload) = frame.split_once(' ').unwrap_or((&frame, ""));
      match event {
        "MSG" => {
          message(db, line, 0, payload, "live", stats)?;
        }
        "HISTORY" => history(db, line, payload, stats)?,
        _ => {}
      }
    }
    Body::Fetched {
      source,
      timestamp,
      nick,
      text,
    } => fetched(db, line, &source, timestamp, &nick, &text, stats)?,
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
  line: &Line,
  payload: &str,
  stats: &mut Stats,
) -> Result<(), Error> {
  let Ok(Value::Array(entries)) = ::serde_json::from_str(payload) else {
    unparsed(db, line.path, line.offset, 0, "HISTORY not a JSON array")?;
    stats.unparsed += 1;
    return Ok(());
  };
  let mut oldest = None;
  for (item, entry) in (1..).zip(entries) {
    let Value::String(frame) = entry else {
      unparsed(
        db,
        line.path,
        line.offset,
        item,
        "HISTORY entry not a frame",
      )?;
      stats.unparsed += 1;
      continue;
    };
    if let Some(payload) = frame.strip_prefix("MSG ")
      && let Some(timestamp) =
        message(db, line, item, payload, "history", stats)?
    {
      oldest = Some(oldest.map_or(timestamp, |o: i64| o.min(timestamp)));
    }
  }
  if let Some(oldest) = oldest {
    db.execute(
      "UPDATE connections SET history_from = min(coalesce(history_from, ?3), ?3)
        WHERE instance = ?1 AND connection = ?2",
      params![line.instance, line.connection as i64, oldest],
    )?;
  }
  Ok(())
}

/// Indexes one `MSG` payload, deduplicating against what's already there.
/// Returns its timestamp, or `None` if it couldn't be parsed.
fn message(
  db: &Transaction,
  line: &Line,
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
      unparsed(db, line.path, line.offset, item, reason)?;
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
    line.received,
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
      params![timestamp, nick, data, line.received],
    )?;
  }
  Ok(Some(timestamp))
}

/// Indexes one backfilled message, unless a message from the same user at
/// the same time is already there.
fn fetched(
  db: &Transaction,
  line: &Line,
  source: &str,
  timestamp: i64,
  nick: &str,
  text: &str,
  stats: &mut Stats,
) -> Result<(), Error> {
  let exists: bool = db.query_row(
    "SELECT EXISTS (SELECT 1 FROM messages
      WHERE timestamp = ?1 AND lower(nick) = lower(?2))",
    params![timestamp, nick],
    |row| row.get(0),
  )?;
  if exists {
    return Ok(());
  }
  let cased: Option<String> = db
    .query_row(
      "SELECT nick FROM messages
        WHERE lower(nick) = lower(?1) AND source NOT LIKE 'backfill:%'
        LIMIT 1",
      [nick],
      |row| row.get(0),
    )
    .optional()?;
  let nick = cased.as_deref().unwrap_or(nick);
  let source = format!("backfill:{source}");
  if insert(db, timestamp, nick, text, "[]", line.received, &source)? {
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

/// Removes backfilled messages a native one at `timestamp` from `nick`
/// supersedes, with their full-text entries.
fn remove_backfilled(
  db: &Transaction,
  timestamp: i64,
  nick: &str,
) -> Result<(), Error> {
  let mut query = db.prepare_cached(
    "SELECT id, data FROM messages
      WHERE timestamp = ?1 AND lower(nick) = lower(?2)
        AND source LIKE 'backfill:%'",
  )?;
  let superseded: Vec<(i64, String)> = query
    .query_map(params![timestamp, nick], |row| {
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
