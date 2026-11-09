use {
  ::journal::{Version, Writer, record::v0::Body},
  ::logger::index::{
    Error, SCHEMA_VERSION, follow, index, open, rebuild, user_version,
  },
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
  let next = Version {
    minor: SCHEMA_VERSION.minor + 1,
    ..SCHEMA_VERSION
  };
  db.pragma_update(None, "user_version", user_version(next))
    .unwrap();
  drop(db);
  let error = open(&root.database()).unwrap_err();
  assert!(matches!(error, Error::Schema(v) if v == next));
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

#[test]
fn backfill_yields_to_native_messages() {
  let root = TempRoot::new();
  let live = |nick: &str, timestamp, data: &str| {
    frame(format!("MSG {}", message(timestamp, nick, data)))
  };
  let fetched = |timestamp, nick: &str, text: &str| Body::Fetched {
    source: "rustlesearch".into(),
    timestamp,
    nick: nick.into(),
    text: text.into(),
  };
  let mut a = Writer::new(root.journal(), "a");
  a.write(100, 1, Body::Open("wss://x".into())).unwrap();
  a.write(101, 1, live("Alice", 100, "one\ntwo")).unwrap();
  drop(a);
  let mut backfill = Writer::new(root.journal(), "backfill-x");
  // The same message as a's, as rustlesearch has it: skipped.
  backfill
    .write(900, 1, fetched(100, "alice", "one two"))
    .unwrap();
  // Alice's casing is known from a; Bob's and Carol's aren't yet.
  backfill
    .write(900, 1, fetched(200, "alice", "only fetched"))
    .unwrap();
  backfill.write(900, 1, fetched(250, "carol", "hi")).unwrap();
  backfill
    .write(900, 1, fetched(300, "bob", "bob says"))
    .unwrap();
  let range = Body::FetchRange {
    source: "rustlesearch".into(),
    from: 50,
    to: 400,
  };
  backfill.write(900, 1, range).unwrap();
  drop(backfill);

  let mut db = open(&root.database()).unwrap();
  let stats = index(&mut db, &root.journal()).unwrap();
  assert_eq!((stats.messages, stats.backfilled), (4, 3));
  let rows = |db: &Connection| -> Vec<(i64, String, String)> {
    messages(db)
      .into_iter()
      .map(|(timestamp, nick, _, _, source)| (timestamp, nick, source))
      .collect()
  };
  let backfill = "backfill:rustlesearch";
  assert_eq!(
    rows(&db),
    [
      (100, "Alice".into(), "live".into()),
      (200, "Alice".into(), backfill.into()),
      (250, "carol".into(), backfill.into()),
      (300, "bob".into(), backfill.into()),
    ]
  );

  // Native messages arrive later: Bob's replaces the backfilled copy, and
  // Carol's other message re-cases hers.
  let mut b = Writer::new(root.journal(), "b");
  b.write(1000, 1, Body::Open("wss://x".into())).unwrap();
  b.write(1001, 1, live("Bob", 300, "bob\nsays")).unwrap();
  b.write(1002, 1, live("Carol", 260, "yo")).unwrap();
  drop(b);
  index(&mut db, &root.journal()).unwrap();
  assert_eq!(
    rows(&db),
    [
      (100, "Alice".into(), "live".into()),
      (200, "Alice".into(), backfill.into()),
      (250, "Carol".into(), backfill.into()),
      (260, "Carol".into(), "live".into()),
      (300, "Bob".into(), "live".into()),
    ]
  );
  // The replaced copy is gone from full-text search too.
  let says = count(
    &db,
    "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'says'",
  );
  assert_eq!(says, 1);
  let ranges = count(
    &db,
    "SELECT count(*) FROM fetch_ranges
      WHERE source = 'rustlesearch' AND start_time = 50 AND end_time = 400",
  );
  assert_eq!(ranges, 1);

  // Rebuilding indexes the files in one go, in path order, and agrees.
  let incremental = messages(&db);
  drop(db);
  let mut db = rebuild(&root.database()).unwrap();
  index(&mut db, &root.journal()).unwrap();
  assert_eq!(messages(&db), incremental);
}

#[test]
fn truncated_timestamps_match_within_their_second() {
  let root = TempRoot::new();
  let live = |nick: &str, timestamp, data: &str| {
    frame(format!("MSG {}", message(timestamp, nick, data)))
  };
  let fetched =
    |source: &str, timestamp, nick: &str, text: &str| Body::Fetched {
      source: source.into(),
      timestamp,
      nick: nick.into(),
      text: text.into(),
    };
  let mut a = Writer::new(root.journal(), "a");
  a.write(100, 1, Body::Open("wss://x".into())).unwrap();
  a.write(101, 1, live("Alice", 5_250, "one\ntwo")).unwrap();
  drop(a);
  let mut linkers = Writer::new(root.journal(), "backfill-l");
  // Alice's message truncated to its second: skipped.
  linkers
    .write(900, 1, fetched("linkers", 5_000, "Alice", "one two"))
    .unwrap();
  // The second before it doesn't match.
  linkers
    .write(900, 1, fetched("linkers", 4_000, "Alice", "earlier"))
    .unwrap();
  linkers
    .write(900, 1, fetched("linkers", 7_000, "Bob", "bob says"))
    .unwrap();
  linkers
    .write(900, 1, fetched("linkers", 8_000, "Carol", "hi"))
    .unwrap();
  drop(linkers);

  let mut db = open(&root.database()).unwrap();
  let stats = index(&mut db, &root.journal()).unwrap();
  assert_eq!((stats.messages, stats.backfilled), (4, 3));

  // rustlesearch's precise copy of Bob's replaces linkers', taking the
  // casing linkers had; a native copy of Carol's replaces hers. Named to
  // sort first, so a rebuild indexes these in the other order.
  let mut rustle = Writer::new(root.journal(), "backfill-0");
  rustle
    .write(950, 1, fetched("rustlesearch", 7_420, "bob", "bob says"))
    .unwrap();
  rustle
    .write(950, 1, fetched("rustlesearch", 6_500, "bob", "first"))
    .unwrap();
  drop(rustle);
  let mut b = Writer::new(root.journal(), "b");
  b.write(1000, 1, Body::Open("wss://x".into())).unwrap();
  b.write(1001, 1, live("Carol", 8_999, "hi")).unwrap();
  drop(b);
  index(&mut db, &root.journal()).unwrap();
  let rows: Vec<(i64, String, String)> = messages(&db)
    .into_iter()
    .map(|(timestamp, nick, _, _, source)| (timestamp, nick, source))
    .collect();
  assert_eq!(
    rows,
    [
      (4_000, "Alice".into(), "backfill:linkers".into()),
      (5_250, "Alice".into(), "live".into()),
      (6_500, "Bob".into(), "backfill:rustlesearch".into()),
      (7_420, "Bob".into(), "backfill:rustlesearch".into()),
      (8_999, "Carol".into(), "live".into()),
    ]
  );
  let says = count(
    &db,
    "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'says'",
  );
  assert_eq!(says, 1);

  // Rebuilding agrees.
  let incremental = messages(&db);
  drop(db);
  let mut db = rebuild(&root.database()).unwrap();
  index(&mut db, &root.journal()).unwrap();
  assert_eq!(messages(&db), incremental);
}

#[test]
fn follows_files_into_archives() {
  use ::logger::compact::compact_day;
  let root = TempRoot::new();
  let live = |nick: &str, timestamp, data: &str| {
    frame(format!("MSG {}", message(timestamp, nick, data)))
  };
  let mut a = Writer::new(root.journal(), "a");
  a.write(100, 1, Body::Open("wss://x".into())).unwrap();
  a.write(101, 1, live("Alice", 101, "one")).unwrap();
  a.write(102, 1, live("Bob", 102, "two")).unwrap();
  drop(a);
  let mut db = open(&root.database()).unwrap();
  index(&mut db, &root.journal()).unwrap();
  assert_eq!(messages(&db).len(), 2);

  // Compacting moves the day into an archive: indexed once more (nothing
  // new), its live checkpoint forgotten, then skipped without being read.
  compact_day(&root.journal(), "1970-01-01", 3)
    .unwrap()
    .unwrap();
  let archived = index(&mut db, &root.journal()).unwrap();
  assert_eq!((archived.lines, archived.messages), (3, 0));
  let paths = |db: &Connection| -> Vec<(String, bool)> {
    db.prepare("SELECT path, digest IS NOT NULL FROM files ORDER BY path")
      .unwrap()
      .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
      .unwrap()
      .map(Result::unwrap)
      .collect()
  };
  let archive_path = "archive/1970/01/1970-01-01.journal.zst".to_owned();
  assert_eq!(paths(&db), [(archive_path.clone(), true)]);
  let idle = index(&mut db, &root.journal()).unwrap();
  assert_eq!(idle.lines, 0);

  // A late file re-merges the archive: its digest changes, so it's read
  // again from the start, and the late message arrives.
  let mut b = Writer::new(root.journal(), "b");
  b.write(103, 1, Body::Open("wss://x".into())).unwrap();
  b.write(104, 1, live("Carol", 104, "three")).unwrap();
  drop(b);
  compact_day(&root.journal(), "1970-01-01", 3)
    .unwrap()
    .unwrap();
  let remerged = index(&mut db, &root.journal()).unwrap();
  assert_eq!((remerged.lines, remerged.messages), (5, 1));
  assert_eq!(paths(&db), [(archive_path, true)]);

  // Rebuilding from archives alone gives the same index.
  let incremental = messages(&db);
  drop(db);
  let mut db = rebuild(&root.database()).unwrap();
  index(&mut db, &root.journal()).unwrap();
  assert_eq!(messages(&db), incremental);
  assert_eq!(incremental.len(), 3);
}
