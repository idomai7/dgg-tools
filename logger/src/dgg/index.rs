//! dgg's index: a channel's journal into SQLite, `index-dgg-<name>.sqlite`
//! (see [`file()`]), for searching and for finding gaps. As every service's
//! (see [`crate::index`]), it's derived data, and each file's progress is
//! checkpointed with its rows. Its paths are relative to the channel's
//! directory, so a channel's messages are deduplicated among themselves
//! alone.
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
  crate::{
    dgg::Dgg,
    index::{Error, Tally, checkpoint, open_at, open_read_only_at, waiting_in},
    service::Service,
  },
  ::logger_journal::{
    Entry, ReadError, Reader, Record, Version, archive, record::v0::Body,
  },
  ::rusqlite::{Connection, OptionalExtension, Transaction, params},
  ::serde_json::Value,
  ::std::{collections::HashMap, io, path::Path, time::Duration},
};

/// Bumped whenever the schema changes; an index at another version must be
/// rebuilt. While the major is 0, every change bumps the minor.
pub const SCHEMA_VERSION: Version = Version { major: 0, minor: 6 };

/// The current schema. Every version's schema is kept in `schemas/index/`;
/// add a new file there (and bump [`SCHEMA_VERSION`]) rather than editing
/// an old one.
const SCHEMA: &str = include_str!("../../schemas/index/v0.6.sql");

/// The file of channel `name`'s index, in the data directory.
pub fn file(name: &str) -> String {
  format!("index-dgg-{name}.sqlite")
}

/// Records are committed in batches of this many, with their checkpoint.
const BATCH: usize = 10_000;

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

impl Tally for Stats {
  fn records(&self) -> u64 {
    self.records
  }

  fn skipped(&self) -> &[(String, String)] {
    &self.skipped
  }

  fn add(&mut self, other: &Self) {
    self.records += other.records;
    self.messages += other.messages;
    self.backfilled += other.backfilled;
    self.unparsed += other.unparsed;
  }

  fn summary(&self) -> String {
    format!(
      "{} records, {} new messages ({} backfilled), {} unparsed",
      self.records, self.messages, self.backfilled, self.unparsed
    )
  }
}

/// Opens the index at `path` only to read it, as it is: it must exist, with
/// this schema. Waits up to `busy` for a writer's lock.
pub fn open_read_only(
  path: &Path,
  busy: Duration,
) -> Result<Connection, Error> {
  open_read_only_at(path, busy, Dgg::NAME, SCHEMA_VERSION)
}

/// Opens the index at `path`, creating it if needed.
pub fn open(path: &Path) -> Result<Connection, Error> {
  open_at(path, Dgg::NAME, SCHEMA, SCHEMA_VERSION)
}

/// How many records in the live and backfill files of the channel in
/// `dir` aren't indexed yet, counting up to `at_most`.
pub fn waiting(
  db: &Connection,
  dir: &Path,
  at_most: u64,
) -> Result<u64, Error> {
  let mut files = Vec::new();
  for root in Dgg::channel_roots(dir, "") {
    for file in root.live_files()? {
      files.push((root.path(&file), file));
    }
  }
  waiting_in(db, files, at_most)
}

/// Indexes everything in the journals of the channel in `dir` that isn't
/// indexed yet: every root's live files, then their archives.
pub fn index(db: &mut Connection, dir: &Path) -> Result<Stats, Error> {
  let mut stats = Stats::default();
  let roots = Dgg::channel_roots(dir, "");
  let mut live = Vec::new();
  let mut files = Vec::new();
  for root in &roots {
    for file in root.live_files()? {
      files.push((root.path(&file), file));
    }
  }
  for (path, file) in files {
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
  let mut archives = Vec::new();
  for root in &roots {
    archives.extend(archive::all(&root.dir)?.into_iter().map(|a| (root, a)));
  }
  for (root, archive) in archives {
    let file = archive.path(&root.dir);
    let path = root.path(&file);
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
