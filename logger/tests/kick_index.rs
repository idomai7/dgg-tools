//! Kick's index: a Kick channel's journal into
//! `index-kick-<slug>.sqlite`.
//! Ids and names are made up.

use {
  ::common::temp_dir::TempDir,
  ::logger::{
    compact::compact_day,
    index::{Error, rebuild, user_version},
    kick::{
      Kick,
      index::{SCHEMA_VERSION, file, index, open, waiting},
    },
    service::Root,
  },
  ::logger_journal::{
    Version,
    layout::{channel_root, kick_root},
    record::kick::v0::Body,
    writer::kick::Writer,
  },
  ::rusqlite::Connection,
  ::serde_json::{Value, json},
  ::std::path::PathBuf,
};

fn journal(dir: &TempDir) -> PathBuf {
  dir.path().join("journal")
}

fn database(dir: &TempDir) -> PathBuf {
  dir.path().join(file("c"))
}

/// Channel `c`'s directory, which its index is of.
fn c(dir: &TempDir) -> PathBuf {
  channel_root(&kick_root(&journal(dir)), "c")
}

/// A frame pushing Kick's event `event` (without its `App\Events\`) with
/// `data`.
fn push(event: &str, data: Value) -> Body {
  let frame = json!({
    "push": {
      "channel": "chatrooms.1.v2",
      "pub": {
        "data": {"event": format!("App\\Events\\{event}"), "data": data.to_string()}
      }
    }
  });
  Body::Frame(frame.to_string())
}

fn user(id: u64, name: &str) -> Value {
  json!({"id": id, "username": name, "slug": name.to_lowercase()})
}

fn message(id: &str, sender: Value, content: &str) -> Value {
  json!({
    "id": id,
    "chatroom_id": 1,
    "content": content,
    "type": "message",
    "created_at": "2026-11-24T12:00:00+00:00",
    "sender": sender,
  })
}

/// A history page of `messages`.
fn page(messages: Vec<Value>) -> Body {
  Body::History {
    cursor: None,
    page: json!({"data": {"messages": messages, "cursor": null}}).to_string(),
  }
}

/// Writes `records`, each `(receive time, body)`, as `instance` in channel
/// `slug`'s journal.
fn write(dir: &TempDir, slug: &str, instance: &str, records: Vec<(i64, Body)>) {
  let root = channel_root(&kick_root(&journal(dir)), slug);
  let mut writer = Writer::new(root, instance, slug);
  for (time, body) in records {
    writer.write(time, 1, body).unwrap();
  }
}

fn count(db: &Connection, sql: &str) -> i64 {
  db.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// A day's records in channel `c`: a message, its deletion, a timeout and
/// an unban, an event that doesn't decode, and a history page with the
/// message again and one more.
fn write_day(dir: &TempDir) {
  let seven = user(7, "Seven");
  write(
    dir,
    "c",
    "k",
    vec![
      (10, Body::Open("wss://one".into())),
      (
        11,
        push("ChatMessageEvent", message("m1", seven.clone(), "hi")),
      ),
      (
        12,
        push(
          "MessageDeletedEvent",
          json!({
            "id": "d1",
            "message": {"id": "m1"},
            "aiModerated": true,
            "violatedRules": ["hate"],
          }),
        ),
      ),
      (
        13,
        push(
          "UserBannedEvent",
          json!({
            "id": "b1",
            "user": seven.clone(),
            "banned_by": user(0, "Moderator"),
            "permanent": false,
            "duration": 10,
            "expires_at": "2026-11-24T12:10:00+00:00",
          }),
        ),
      ),
      (
        14,
        push(
          "UserUnbannedEvent",
          json!({
            "id": "u1",
            "user": seven.clone(),
            "unbanned_by": user(0, "Moderator"),
            "permanent": false,
          }),
        ),
      ),
      (15, push("UserBannedEvent", json!({"id": "b2"}))),
      (
        16,
        page(vec![
          message("m1", seven.clone(), "hi"),
          message("m2", user(8, "Eight"), "missed"),
        ]),
      ),
    ],
  );
}

#[test]
fn indexes_messages_deletions_and_bans() {
  let dir = TempDir::new("logger-kick-index");
  write_day(&dir);
  let mut db = open(&database(&dir)).unwrap();
  let stats = index(&mut db, &c(&dir)).unwrap();
  assert_eq!(
    (
      stats.records,
      stats.messages,
      stats.recovered,
      stats.deletions,
      stats.bans,
      stats.unparsed
    ),
    (7, 2, 1, 1, 2, 1)
  );
  let messages: Vec<(String, String, i64, i64, String, String, i64)> = db
    .prepare(
      "SELECT id, channel, received, user_id, username, source, sent
        FROM messages ORDER BY id",
    )
    .unwrap()
    .query_map([], |row| {
      Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
      ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect();
  let sent = 1_795_521_600_000;
  assert_eq!(
    messages,
    [
      (
        "m1".into(),
        "c".into(),
        11,
        7,
        "Seven".into(),
        "live".into(),
        sent
      ),
      (
        "m2".into(),
        "c".into(),
        16,
        8,
        "Eight".into(),
        "history".into(),
        sent
      ),
    ]
  );
  let deletion: (String, i64, bool, String) = db
    .query_row(
      "SELECT message_id, received, ai, rules FROM deletions",
      [],
      |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .unwrap();
  assert_eq!(deletion, ("m1".into(), 12, true, r#"["hate"]"#.into()));
  let bans: Vec<(String, String, Option<String>, Option<i64>)> = db
    .prepare("SELECT id, kind, by, minutes FROM bans ORDER BY received")
    .unwrap()
    .query_map([], |row| {
      Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect();
  assert_eq!(
    bans,
    [
      (
        "b1".into(),
        "timeout".into(),
        Some("Moderator".into()),
        Some(10)
      ),
      ("u1".into(), "unban".into(), Some("Moderator".into()), None),
    ]
  );
  let reason: String = db
    .query_row("SELECT reason FROM unparsed", [], |row| row.get(0))
    .unwrap();
  assert!(
    reason.starts_with("App\\Events\\UserBannedEvent: "),
    "{reason}"
  );

  // Nothing new: nothing indexed twice.
  let again = index(&mut db, &c(&dir)).unwrap();
  assert_eq!((again.records, again.messages), (0, 0));
}

#[test]
fn merges_what_several_captures_saw() {
  let dir = TempDir::new("logger-kick-index");
  write_day(&dir);
  // Another capture received m1 earlier, and m2 live, after the page.
  let seven = user(7, "Seven");
  write(
    &dir,
    "c",
    "l",
    vec![
      (9, push("ChatMessageEvent", message("m1", seven, "hi"))),
      (
        20,
        push("ChatMessageEvent", message("m2", user(8, "Eight"), "x")),
      ),
      (
        21,
        push(
          "MessageDeletedEvent",
          json!({"id": "d1", "message": {"id": "m1"}}),
        ),
      ),
    ],
  );
  let mut db = open(&database(&dir)).unwrap();
  index(&mut db, &c(&dir)).unwrap();
  let row = |id: &str| -> (i64, String) {
    db.query_row(
      "SELECT received, source FROM messages WHERE id = ?1",
      [id],
      |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
  };
  assert_eq!(row("m1"), (9, "live".into()));
  assert_eq!(row("m2"), (20, "live".into()));
  assert_eq!(count(&db, "SELECT count(*) FROM deletions"), 1);
  assert_eq!(count(&db, "SELECT received FROM deletions"), 12);
}

#[test]
fn reads_archives_once() {
  let dir = TempDir::new("logger-kick-index");
  write_day(&dir);
  let mut db = open(&database(&dir)).unwrap();
  index(&mut db, &c(&dir)).unwrap();
  compact_day(&Root::at(c(&dir)), "1970-01-01", 3)
    .unwrap()
    .unwrap();
  // The archive is read through, adding nothing, and the live file's
  // checkpoint goes with it.
  let stats = index(&mut db, &c(&dir)).unwrap();
  assert_eq!((stats.records, stats.messages), (7, 0));
  assert_eq!(stats.forgotten.len(), 1);
  assert!(stats.forgotten[0].starts_with("live/1970-01-01/"));
  let paths: Vec<String> = db
    .prepare("SELECT path FROM files")
    .unwrap()
    .query_map([], |row| row.get(0))
    .unwrap()
    .map(Result::unwrap)
    .collect();
  assert_eq!(paths, ["archive/1970/01/1970-01-01.journal.zst"]);
  let again = index(&mut db, &c(&dir)).unwrap();
  assert_eq!(again.records, 0);
  // Rebuilt from the archive alone, it's the same.
  drop(db);
  let mut fresh = rebuild::<Kick>(&database(&dir)).unwrap();
  let stats = index(&mut fresh, &c(&dir)).unwrap();
  assert_eq!((stats.messages, stats.deletions, stats.bans), (2, 1, 2));
}

#[test]
fn counts_what_is_waiting() {
  let dir = TempDir::new("logger-kick-index");
  write_day(&dir);
  // Another channel's, which has an index of its own.
  write(&dir, "d", "k", vec![(10, Body::Open("wss://one".into()))]);
  let mut db = open(&database(&dir)).unwrap();
  assert_eq!(waiting(&db, &c(&dir), 100).unwrap(), 7);
  index(&mut db, &c(&dir)).unwrap();
  assert_eq!(waiting(&db, &c(&dir), 100).unwrap(), 0);
}

#[test]
fn keeps_each_channel_apart() {
  let dir = TempDir::new("logger-kick-index");
  write_day(&dir);
  let seven = user(7, "Seven");
  write(
    &dir,
    "d",
    "k",
    vec![(
      9,
      push("ChatMessageEvent", message("m9", seven, "elsewhere")),
    )],
  );
  let mut db = open(&database(&dir)).unwrap();
  let stats = index(&mut db, &c(&dir)).unwrap();
  assert_eq!(stats.messages, 2);
  let d = channel_root(&kick_root(&journal(&dir)), "d");
  let mut other = open(&dir.path().join(file("d"))).unwrap();
  assert_eq!(index(&mut other, &d).unwrap().messages, 1);
  let channel: String = other
    .query_row("SELECT channel FROM messages", [], |row| row.get(0))
    .unwrap();
  assert_eq!(channel, "d");
  assert_eq!(count(&db, "SELECT count(*) FROM messages"), 2);
}

#[test]
fn an_index_at_another_schema_is_refused() {
  let dir = TempDir::new("logger-kick-index");
  let db = open(&database(&dir)).unwrap();
  let old = Version { major: 0, minor: 9 };
  db.pragma_update(None, "user_version", user_version(old))
    .unwrap();
  drop(db);
  assert!(matches!(
    open(&database(&dir)),
    Err(Error::Schema { service: "kick", found, .. }) if found == old
  ));
  let db = rebuild::<Kick>(&database(&dir)).unwrap();
  let version: i64 = db
    .pragma_query_value(None, "user_version", |row| row.get(0))
    .unwrap();
  assert_eq!(version, user_version(SCHEMA_VERSION));
}
