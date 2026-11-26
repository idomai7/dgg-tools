//! Kick's index: a Kick channel's journal into SQLite,
//! `index-kick-<slug>.sqlite` (see [`file()`]), apart from dgg's indexes,
//! with schema versions of its own. It's what a user's history in the
//! channel is looked up in.
//!
//! As dgg's, it's derived data, rebuilt from the journal at will, and each
//! file's progress is checkpointed in the same transaction as its rows.
//! Archives are found through their manifests. Files are named by their
//! paths in the channel's directory, `live/...` and `archive/...`.
//!
//! Rows are keyed by Kick's own ids, so the same message, deletion or ban
//! seen by captures on several hosts is kept once, with the earliest
//! receive time:
//!
//! - messages, from chat and from the history pages a gap was filled with
//!   (`source = history`, until it's seen live);
//! - deletions, by the deleted message's id;
//! - bans, timeouts and unbans, by their own id; one without an id (Kick's
//!   are optional) is kept as often as it's seen.
//!
//! An event that doesn't decode, a history page that doesn't parse, and a
//! record that isn't Kick's are noted in `unparsed`.

use {
  crate::{
    index::{
      Error, Tally, checkpoint, forget_gone, open_at, open_read_only_at,
      relative, waiting_in,
    },
    kick::Kick,
    service::Service,
  },
  ::kick_api::{
    realtime::{Event, Frame},
    web,
  },
  ::logger_journal::{
    Entry, ReadError, Reader, Record, Version, archive,
    layout::{channel_root, live_files},
    record::kick::v0::Body,
  },
  ::rusqlite::{Connection, Transaction, params},
  ::std::{io, path::Path, time::Duration},
};

/// Bumped whenever the schema changes; an index at another version must be
/// rebuilt. While the major is 0, every change bumps the minor.
pub const SCHEMA_VERSION: Version = Version { major: 0, minor: 1 };

/// The current schema. Every version's schema is kept in
/// `schemas/index-kick/`; add a new file there (and bump
/// [`SCHEMA_VERSION`]) rather than editing an old one.
const SCHEMA: &str = include_str!("../../schemas/index-kick/v0.1.sql");

/// The file of channel `slug`'s index, in the data directory.
pub fn file(slug: &str) -> String {
  format!("index-kick-{slug}.sqlite")
}

/// The file of the index of every channel's, in the data directory, from
/// before each channel had its own; [`migrate`](crate::migrate) deletes
/// it.
pub const OLD_FILE: &str = "index-kick.sqlite";

/// Records are committed in batches of this many, with their checkpoint.
const BATCH: usize = 10_000;

/// What one [`index`] run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  /// Journal records read.
  pub records: u64,
  /// Messages added (not counting ones already there).
  pub messages: u64,
  /// Of those, how many came from history pages.
  pub recovered: u64,
  /// Deletions added.
  pub deletions: u64,
  /// Bans, timeouts and unbans added.
  pub bans: u64,
  /// Records, events or history pages that couldn't be indexed.
  pub unparsed: u64,
  /// Files skipped because their header couldn't be read, with why.
  pub skipped: Vec<(String, String)>,
  /// Live files gone from the journal, forgotten.
  pub forgotten: Vec<String>,
}

impl Tally for Stats {
  fn records(&self) -> u64 {
    self.records
  }

  fn skipped(&self) -> &[(String, String)] {
    &self.skipped
  }

  fn forgotten(&self) -> &[String] {
    &self.forgotten
  }

  fn add(&mut self, other: &Self) {
    self.records += other.records;
    self.messages += other.messages;
    self.recovered += other.recovered;
    self.deletions += other.deletions;
    self.bans += other.bans;
    self.unparsed += other.unparsed;
  }

  fn summary(&self) -> String {
    format!(
      "{} records, {} new messages ({} from history), {} deletions, {} \
       bans and unbans, {} unparsed",
      self.records,
      self.messages,
      self.recovered,
      self.deletions,
      self.bans,
      self.unparsed
    )
  }
}

/// Opens Kick's index at `path`, creating it if needed.
pub fn open(path: &Path) -> Result<Connection, Error> {
  open_at(path, Kick::NAME, SCHEMA, SCHEMA_VERSION)
}

/// Opens Kick's index at `path` only to read it, as it is: it must exist,
/// with this schema. Waits up to `busy` for a writer's lock.
pub fn open_read_only(
  path: &Path,
  busy: Duration,
) -> Result<Connection, Error> {
  open_read_only_at(path, busy, Kick::NAME, SCHEMA_VERSION)
}

/// How many records in the live files of the channel in `dir` aren't
/// indexed yet, counting up to `at_most`.
pub fn waiting(
  db: &Connection,
  dir: &Path,
  at_most: u64,
) -> Result<u64, Error> {
  let files = live_files(dir)?;
  let files = files.into_iter().map(|file| (relative(dir, &file), file));
  waiting_in(db, files.collect(), at_most)
}

/// Indexes everything in the journal of the channel in `dir`, named by
/// its slug, that isn't indexed yet.
pub fn index(db: &mut Connection, dir: &Path) -> Result<Stats, Error> {
  let mut stats = Stats::default();
  let mut live = Vec::new();
  let slug = dir.file_name().unwrap_or_default().to_string_lossy();
  for file in live_files(dir)? {
    let path = relative(dir, &file);
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
    let file = Origin {
      channel: &slug,
      path: &path,
    };
    index_file(db, &file, None, reader, &mut stats)?;
  }
  for archive in archive::all(dir)? {
    let file = archive.path(dir);
    let path = relative(dir, &file);
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
    let origin = Origin {
      channel: &slug,
      path: &path,
    };
    index_file(db, &origin, Some(&archive.sha256), reader, &mut stats)?;
  }
  stats.forgotten = forget_gone(db, &["live/"], &live)?;
  Ok(stats)
}

/// An index, in memory, of what channel `slug`'s live files hold past
/// their checkpoints in `db` (all of a file it has none for, or of every
/// file without `db`): what Kick's index hasn't reached yet, small enough
/// to build for each query.
pub(crate) fn recent(
  db: Option<&Connection>,
  journal: &Path,
  slug: &str,
) -> Result<Connection, Error> {
  let mut recent = Connection::open_in_memory()?;
  recent.execute_batch(SCHEMA)?;
  let root = channel_root(&Kick::dir(journal), slug);
  let mut stats = Stats::default();
  for file in live_files(&root)? {
    let path = relative(&root, &file);
    let offset = match db {
      Some(db) => checkpoint(db, &path)?.0,
      None => 0,
    };
    let reader = match Reader::open(&file, offset) {
      Ok(reader) => reader,
      Err(ReadError::Io(e)) if e.kind() != io::ErrorKind::NotFound => {
        return Err(e.into());
      }
      // Compacted away meanwhile, or one a round would skip.
      Err(_) => continue,
    };
    let origin = Origin {
      channel: slug,
      path: &path,
    };
    index_file(&mut recent, &origin, None, reader, &mut stats)?;
  }
  Ok(recent)
}

/// The file being indexed: its channel, and its path as the index names it.
struct Origin<'a> {
  channel: &'a str,
  path: &'a str,
}

/// Where a record came from, and when it was received.
struct At<'a> {
  file: &'a Origin<'a>,
  offset: u64,
  received: i64,
}

fn index_file(
  db: &mut Connection,
  file: &Origin,
  digest: Option<&str>,
  mut reader: Reader,
  stats: &mut Stats,
) -> Result<(), Error> {
  loop {
    let transaction = db.transaction()?;
    let mut records = 0;
    for entry in reader.by_ref().take(BATCH) {
      records += 1;
      match entry {
        Ok(Entry {
          offset,
          record: Record::V0(_),
          ..
        }) => {
          let why = "a dgg record, in Kick's";
          unparsed(&transaction, file.path, offset, 0, why)?;
          stats.unparsed += 1;
        }
        Ok(Entry {
          offset,
          record: Record::Kick0(record),
          ..
        }) => {
          let at = At {
            file,
            offset,
            received: record.receive_time,
          };
          match record.body {
            Body::Frame(raw) => {
              let frame = Frame::new(record.connection, raw);
              for (item, event) in (1..).zip(frame.events()) {
                index_event(&transaction, &at, item, event, stats)?;
              }
            }
            Body::History { page, .. } => {
              index_page(&transaction, &at, &page, stats)?;
            }
            Body::Open(_) | Body::Close(_) | Body::Binary(_) => {}
          }
        }
        Err(ReadError::Record { offset, error }) => {
          unparsed(&transaction, file.path, offset, 0, &format!("{error:?}"))?;
          stats.unparsed += 1;
        }
        Err(ReadError::Io(e)) => return Err(e.into()),
        Err(e) => return Err(io::Error::other(format!("{e:?}")).into()),
      }
    }
    transaction.execute(
      "INSERT INTO files (path, offset, digest) VALUES (?1, ?2, ?3)
        ON CONFLICT (path) DO UPDATE SET
          offset = excluded.offset, digest = excluded.digest",
      params![file.path, reader.offset() as i64, digest],
    )?;
    transaction.commit()?;
    stats.records += records;
    if records < BATCH as u64 {
      return Ok(());
    }
  }
}

/// Indexes one event of a frame, the `item`th, counting from 1.
fn index_event(
  db: &Transaction,
  at: &At,
  item: u64,
  event: Event,
  stats: &mut Stats,
) -> Result<(), Error> {
  let channel = at.file.channel;
  match event {
    Event::ChatMessageEvent(message) => {
      let new = Said {
        id: &message.id,
        sent: message.created_at,
        user_id: message.sender.id,
        username: &message.sender.username,
        content: &message.content,
        r#type: message.r#type.as_deref(),
      };
      if insert_message(db, at, &new, "live")? {
        stats.messages += 1;
      }
    }
    Event::MessageDeletedEvent(deleted) => {
      let rules = deleted
        .violated_rules
        .map(|rules| ::serde_json::Value::from(rules).to_string());
      let inserted = db.execute(
        "INSERT OR IGNORE INTO deletions
          (message_id, id, channel, received, ai, rules)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
          deleted.message.id,
          deleted.id,
          channel,
          at.received,
          deleted.ai_moderated.unwrap_or(false),
          rules
        ],
      )?;
      if inserted == 1 {
        stats.deletions += 1;
      } else {
        db.execute(
          "UPDATE deletions SET received = min(received, ?2)
            WHERE message_id = ?1",
          params![deleted.message.id, at.received],
        )?;
      }
    }
    Event::UserBannedEvent(banned) => {
      let kind = if banned.permanent { "ban" } else { "timeout" };
      let ban = Ban {
        id: banned.id.as_deref(),
        kind,
        user_id: banned.user.id,
        username: &banned.user.username,
        by: banned.banned_by.as_ref().map(|by| by.username.as_str()),
        minutes: banned.duration,
        until: banned.expires_at,
        permanent: None,
      };
      if insert_ban(db, at, &ban)? {
        stats.bans += 1;
      }
    }
    Event::UserUnbannedEvent(unbanned) => {
      let ban = Ban {
        id: unbanned.id.as_deref(),
        kind: "unban",
        user_id: unbanned.user.id,
        username: &unbanned.user.username,
        by: unbanned.unbanned_by.as_ref().map(|by| by.username.as_str()),
        minutes: None,
        until: None,
        permanent: unbanned.permanent,
      };
      if insert_ban(db, at, &ban)? {
        stats.bans += 1;
      }
    }
    Event::Unreadable { event, error, .. } => {
      let why = format!("{event}: {error}");
      unparsed(db, at.file.path, at.offset, item, &why)?;
      stats.unparsed += 1;
    }
    _ => {}
  }
  Ok(())
}

/// Indexes the messages of a history page a gap was filled with.
fn index_page(
  db: &Transaction,
  at: &At,
  page: &str,
  stats: &mut Stats,
) -> Result<(), Error> {
  let page = match web::parse(page) {
    Ok(page) => page,
    Err(e) => {
      let why = format!("history page: {e}");
      unparsed(db, at.file.path, at.offset, 0, &why)?;
      stats.unparsed += 1;
      return Ok(());
    }
  };
  for message in &page.messages {
    let new = Said {
      id: &message.id,
      sent: message.created_at,
      user_id: message.sender.id,
      username: &message.sender.username,
      content: &message.content,
      r#type: message.r#type.as_deref(),
    };
    if insert_message(db, at, &new, "history")? {
      stats.messages += 1;
      stats.recovered += 1;
    }
  }
  Ok(())
}

/// A message to index.
struct Said<'a> {
  id: &'a str,
  sent: Option<i64>,
  user_id: u64,
  username: &'a str,
  content: &'a str,
  r#type: Option<&'a str>,
}

/// Inserts a message, from `source`, unless it's already there; whether it
/// was. One already there takes the earliest receive time from the same
/// source, and a live sighting replaces a history one.
fn insert_message(
  db: &Transaction,
  at: &At,
  said: &Said,
  source: &str,
) -> Result<bool, Error> {
  let inserted = db.execute(
    "INSERT OR IGNORE INTO messages
      (id, channel, sent, received, user_id, username, content, type, source)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    params![
      said.id,
      at.file.channel,
      said.sent,
      at.received,
      said.user_id as i64,
      said.username,
      said.content,
      said.r#type,
      source
    ],
  )?;
  if inserted == 0 {
    db.execute(
      "UPDATE messages SET
          received = CASE
            WHEN source = ?3 THEN min(received, ?2)
            WHEN ?3 = 'live' THEN ?2
            ELSE received END,
          source = CASE WHEN ?3 = 'live' THEN 'live' ELSE source END,
          sent = coalesce(sent, ?4)
        WHERE id = ?1",
      params![said.id, at.received, source, said.sent],
    )?;
  }
  Ok(inserted == 1)
}

/// A ban, timeout or unban to index.
struct Ban<'a> {
  id: Option<&'a str>,
  kind: &'a str,
  user_id: u64,
  username: &'a str,
  by: Option<&'a str>,
  minutes: Option<u64>,
  until: Option<i64>,
  permanent: Option<bool>,
}

/// Inserts a ban, timeout or unban, unless one with its id is already
/// there; whether it was. One already there takes the earliest receive
/// time.
fn insert_ban(db: &Transaction, at: &At, ban: &Ban) -> Result<bool, Error> {
  let inserted = db.execute(
    "INSERT OR IGNORE INTO bans
      (id, channel, received, kind, user_id, username, by, minutes, until,
        permanent)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    params![
      ban.id,
      at.file.channel,
      at.received,
      ban.kind,
      ban.user_id as i64,
      ban.username,
      ban.by,
      ban.minutes.map(|minutes| minutes as i64),
      ban.until,
      ban.permanent
    ],
  )?;
  if inserted == 0 {
    db.execute(
      "UPDATE bans SET received = min(received, ?2) WHERE id = ?1",
      params![ban.id, at.received],
    )?;
  }
  Ok(inserted == 1)
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
