use {
  ::journal::{Writer, record::v1::Body},
  ::logger::index::{Error, SCHEMA_VERSION, follow, index, open, rebuild},
  ::rusqlite::Connection,
  ::std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
  },
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("logger-index-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    Self(dir)
  }

  fn journal(&self) -> PathBuf {
    self.0.join("journal")
  }

  fn database(&self) -> PathBuf {
    self.0.join("index.sqlite")
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn message(timestamp: i64, nick: &str, data: &str) -> String {
  ::serde_json::json!({
    "nick": nick,
    "features": ["subscriber"],
    "timestamp": timestamp,
    "data": data,
  })
  .to_string()
}

fn frame(text: String) -> Body {
  Body::Frame(text)
}

/// `(timestamp, nick, data, received, source)` for every message.
fn messages(db: &Connection) -> Vec<(i64, String, String, i64, String)> {
  let mut query = db
    .prepare(
      "SELECT timestamp, nick, data, received, source FROM messages
        ORDER BY timestamp",
    )
    .unwrap();
  query
    .query_map([], |row| {
      Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
      ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn count(db: &Connection, sql: &str) -> i64 {
  db.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// Two instances whose live messages and HISTORY replays overlap.
fn write_journal(root: &TempRoot) {
  let history = |entries: &[String]| {
    let entries: Vec<_> = entries
      .iter()
      .map(|entry| ::serde_json::Value::String(entry.clone()))
      .collect();
    frame(format!("HISTORY {}", ::serde_json::Value::Array(entries)))
  };
  let mut a = Writer::new(root.journal(), "a");
  a.write(100, 1, Body::Open("wss://x".into())).unwrap();
  let entries = [
    format!("MSG {}", message(10, "old", "before a started")),
    format!("MSG {}", message(20, "x", "hello world")),
    "JOIN {}".into(),
  ];
  a.write(101, 1, history(&entries)).unwrap();
  // A non-frame HISTORY entry is unparsed; the rest of the frame isn't lost.
  a.write(102, 1, frame("HISTORY [5]".into())).unwrap();
  // The live copy of a message HISTORY already had.
  a.write(
    103,
    1,
    frame(format!("MSG {}", message(20, "x", "hello world"))),
  )
  .unwrap();
  a.write(104, 1, frame("MSG {not json".into())).unwrap();
  a.write(105, 1, frame(format!("MSG {}", message(30, "y", "second"))))
    .unwrap();
  a.write(106, 1, frame("JOIN {}".into())).unwrap();
  a.write(107, 1, Body::Close("handoff to b".into())).unwrap();
  drop(a);

  let mut b = Writer::new(root.journal(), "b");
  b.write(90, 1, Body::Open("wss://x".into())).unwrap();
  // b saw "second" live earlier than a did, and replays "hello world".
  b.write(91, 1, frame(format!("MSG {}", message(30, "y", "second"))))
    .unwrap();
  let entries = [format!("MSG {}", message(20, "x", "hello world"))];
  b.write(92, 1, history(&entries)).unwrap();
}

#[test]
fn indexes_and_deduplicates() {
  let root = TempRoot::new();
  write_journal(&root);
  let mut db = open(&root.database()).unwrap();
  let stats = index(&mut db, &root.journal()).unwrap();
  assert_eq!((stats.lines, stats.messages, stats.unparsed), (11, 3, 2));
  assert_eq!(stats.skipped, []);
  assert_eq!(
    messages(&db),
    [
      (
        10,
        "old".into(),
        "before a started".into(),
        101,
        "history".into()
      ),
      // Seen in HISTORY first, then live: kept as live.
      (20, "x".into(), "hello world".into(), 103, "live".into()),
      // Seen live by both instances: the earliest receive time wins.
      (30, "y".into(), "second".into(), 91, "live".into()),
    ]
  );
  let features: String = db
    .query_row(
      "SELECT features FROM messages WHERE nick = 'y'",
      [],
      |row| row.get(0),
    )
    .unwrap();
  assert_eq!(features, r#"["subscriber"]"#);
  let unparsed = count(&db, "SELECT count(*) FROM unparsed");
  assert_eq!(unparsed, 2);
  let open_connections =
    count(&db, "SELECT count(*) FROM connections WHERE closed IS NULL");
  let handoffs = count(
    &db,
    "SELECT count(*) FROM connections WHERE close_reason = 'handoff to b'",
  );
  assert_eq!((open_connections, handoffs), (1, 1));
  let found = count(
    &db,
    "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'world'",
  );
  assert_eq!(found, 1);
}

#[test]
fn resumes_from_checkpoints() {
  let root = TempRoot::new();
  write_journal(&root);
  let mut db = open(&root.database()).unwrap();
  index(&mut db, &root.journal()).unwrap();
  let again = index(&mut db, &root.journal()).unwrap();
  assert_eq!((again.lines, again.messages), (0, 0));

  // b keeps writing; only the new line is read.
  let mut b = Writer::new(root.journal(), "b");
  b.write(93, 1, frame(format!("MSG {}", message(40, "z", "third"))))
    .unwrap();
  drop(b);
  let more = index(&mut db, &root.journal()).unwrap();
  assert_eq!((more.lines, more.messages), (1, 1));
  assert_eq!(messages(&db).len(), 4);

  // A rebuild from scratch gives the same index.
  let incremental = messages(&db);
  drop(db);
  let mut db = rebuild(&root.database()).unwrap();
  index(&mut db, &root.journal()).unwrap();
  assert_eq!(messages(&db), incremental);
}

#[test]
fn refuses_other_schema_versions() {
  let root = TempRoot::new();
  let db = open(&root.database()).unwrap();
  db.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
    .unwrap();
  drop(db);
  let error = open(&root.database()).unwrap_err();
  assert!(matches!(error, Error::Schema(v) if v == SCHEMA_VERSION + 1));
  assert!(rebuild(&root.database()).is_ok());
}

#[test]
fn follows_new_lines() {
  use ::std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
  };
  let root = TempRoot::new();
  write_journal(&root);
  let running = Arc::new(AtomicBool::new(true));
  let follower = {
    let (database, journal) = (root.database(), root.journal());
    let running = running.clone();
    ::std::thread::spawn(move || {
      let mut db = open(&database).unwrap();
      let mut messages = 0;
      follow(
        &mut db,
        &journal,
        Duration::from_millis(10),
        || running.load(Ordering::Relaxed),
        |round| messages += round.unwrap().messages,
      )
      .unwrap();
      messages
    })
  };

  let reader = open(&root.database()).unwrap();
  let wait_for = |expected| {
    let deadline = Instant::now() + Duration::from_secs(5);
    while count(&reader, "SELECT count(*) FROM messages") != expected {
      assert!(Instant::now() < deadline, "never reached {expected}");
      ::std::thread::sleep(Duration::from_millis(10));
    }
  };
  wait_for(3);
  let mut b = Writer::new(root.journal(), "b");
  b.write(93, 1, frame(format!("MSG {}", message(40, "z", "third"))))
    .unwrap();
  b.sync().unwrap();
  wait_for(4);
  running.store(false, Ordering::Relaxed);
  assert_eq!(follower.join().unwrap(), 4);
}
