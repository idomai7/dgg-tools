//! A user's history in a Kick channel, for kick-mod: their messages (each
//! with its deletion, if it was deleted), and the bans, timeouts and unbans
//! on them, by their user id, which stays when they rename.
//!
//! It's read from Kick's index and from what the index hasn't reached yet:
//! the channel's live files past its checkpoints, indexed in memory for
//! the query (see [`recent`]). Both are read in one snapshot of the index,
//! so a round finishing meanwhile can't count anything twice.
//!
//! As JSON:
//!
//! ```js
//! {
//!   channel, user_id,
//!   username,        // the newest name seen, or null
//!   messages: [      // oldest first, the newest 500
//!     {id, sent, received, content, source, deleted: {received, ai} | null}
//!   ],
//!   earlier,         // how many older messages were left out
//!   events: [        // oldest first
//!     {id, received, kind, by, minutes, until, permanent}
//!   ]
//! }
//! ```
//!
//! Times are unix ms: `sent` when Kick says (whole seconds, or null),
//! `received` the earliest any capture received it. `source` is `live`, or
//! `history` for a message only a gap's history page had. An event's
//! `kind` is `ban`, `timeout` or `unban`.

use {
  crate::{
    index::Error,
    kick::index::{self, recent},
  },
  ::rusqlite::{Connection, OptionalExtension, params},
  ::serde_json::{Value, json},
  ::std::{collections::HashMap, path::Path, time::Duration},
};

/// The most messages a lookup returns: the newest.
pub const MAX_MESSAGES: usize = 500;

/// How long a lookup waits for an index round's lock.
const BUSY: Duration = Duration::from_secs(5);

/// One of their messages.
struct Said {
  id: String,
  sent: Option<i64>,
  received: i64,
  content: String,
  username: String,
  source: String,
}

/// A ban, timeout or unban on them.
struct Happened {
  id: Option<String>,
  received: i64,
  kind: String,
  username: String,
  by: Option<String>,
  minutes: Option<i64>,
  until: Option<i64>,
  permanent: Option<bool>,
}

/// The history of `user_id` in channel `slug`, from Kick's index at
/// `index_path` (if there is one yet) and the journal at `journal_root`, as
/// JSON (see the [module docs](self)).
pub fn user(
  index_path: &Path,
  journal_root: &Path,
  slug: &str,
  user_id: u64,
) -> Result<Value, Error> {
  let main = match index_path.exists() {
    true => Some(index::open_read_only(index_path, BUSY)?),
    false => None,
  };
  // One snapshot of the index for its checkpoints and its rows.
  let snapshot = main
    .as_ref()
    .map(Connection::unchecked_transaction)
    .transpose()?;
  let recent = recent(main.as_ref(), journal_root, slug)?;
  let dbs: Vec<&Connection> = main.iter().chain([&recent]).collect();

  let mut messages: HashMap<String, Said> = HashMap::new();
  let mut events: Vec<Happened> = Vec::new();
  for db in &dbs {
    for said in their_messages(db, slug, user_id)? {
      match messages.get_mut(&said.id) {
        Some(known) => {
          if said.source == "live" {
            known.source = said.source;
          }
          known.received = known.received.min(said.received);
          known.sent = known.sent.or(said.sent);
        }
        None => {
          messages.insert(said.id.clone(), said);
        }
      }
    }
    for happened in their_events(db, slug, user_id)? {
      let known = happened.id.is_some()
        && events.iter().any(|event| event.id == happened.id);
      if !known {
        events.push(happened);
      }
    }
  }
  let mut messages: Vec<Said> = messages.into_values().collect();
  messages
    .sort_by_key(|said| (said.sent.unwrap_or(said.received), said.received));
  events.sort_by_key(|happened| happened.received);
  let username = messages
    .iter()
    .map(|said| (said.received, &said.username))
    .chain(events.iter().map(|event| (event.received, &event.username)))
    .max_by_key(|(received, _)| *received)
    .map(|(_, username)| username.clone());
  let earlier = messages.len().saturating_sub(MAX_MESSAGES);
  let mut shown = Vec::new();
  for said in messages.into_iter().skip(earlier) {
    let deleted = deletion(&dbs, &said.id)?;
    shown.push(json!({
      "id": said.id,
      "sent": said.sent,
      "received": said.received,
      "content": said.content,
      "source": said.source,
      "deleted": deleted.map(|(received, ai)| json!({
        "received": received,
        "ai": ai,
      })),
    }));
  }
  drop(snapshot);
  let events: Vec<Value> = events
    .into_iter()
    .map(|event| {
      json!({
        "id": event.id,
        "received": event.received,
        "kind": event.kind,
        "by": event.by,
        "minutes": event.minutes,
        "until": event.until,
        "permanent": event.permanent,
      })
    })
    .collect();
  Ok(json!({
    "channel": slug,
    "user_id": user_id,
    "username": username,
    "messages": shown,
    "earlier": earlier,
    "events": events,
  }))
}

fn their_messages(
  db: &Connection,
  slug: &str,
  user_id: u64,
) -> Result<Vec<Said>, Error> {
  let mut query = db.prepare(
    "SELECT id, sent, received, content, username, source FROM messages
      WHERE channel = ?1 AND user_id = ?2",
  )?;
  let rows = query.query_map(params![slug, user_id as i64], |row| {
    Ok(Said {
      id: row.get(0)?,
      sent: row.get(1)?,
      received: row.get(2)?,
      content: row.get(3)?,
      username: row.get(4)?,
      source: row.get(5)?,
    })
  })?;
  Ok(rows.collect::<Result<_, _>>()?)
}

fn their_events(
  db: &Connection,
  slug: &str,
  user_id: u64,
) -> Result<Vec<Happened>, Error> {
  let mut query = db.prepare(
    "SELECT id, received, kind, username, by, minutes, until, permanent
      FROM bans WHERE channel = ?1 AND user_id = ?2",
  )?;
  let rows = query.query_map(params![slug, user_id as i64], |row| {
    Ok(Happened {
      id: row.get(0)?,
      received: row.get(1)?,
      kind: row.get(2)?,
      username: row.get(3)?,
      by: row.get(4)?,
      minutes: row.get(5)?,
      until: row.get(6)?,
      permanent: row.get(7)?,
    })
  })?;
  Ok(rows.collect::<Result<_, _>>()?)
}

/// When message `id` was first seen deleted, and whether by Kick's AI, if
/// any of `dbs` has its deletion.
fn deletion(
  dbs: &[&Connection],
  id: &str,
) -> Result<Option<(i64, bool)>, Error> {
  let mut found: Option<(i64, bool)> = None;
  for db in dbs {
    let row = db
      .query_row(
        "SELECT received, ai FROM deletions WHERE message_id = ?1",
        [id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
      )
      .optional()?;
    if let Some((received, ai)) = row
      && found.is_none_or(|(known, _)| received < known)
    {
      found = Some((received, ai));
    }
  }
  Ok(found)
}
