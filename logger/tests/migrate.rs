//! Migrating archives of a record per line to records ending at RS LF.

use {
  ::common::temp_dir::TempDir,
  ::logger::{
    index::{SCHEMA_VERSION, index, open, user_version},
    migrate::{FROM_SCHEMA, Report, migrate},
  },
  ::logger_journal::{
    archive::{self, Archive},
    layout::{archive_file, backfill_root},
  },
  ::rusqlite::Connection,
  ::sha2::{Digest, Sha256},
  ::std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
  },
};

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn message(timestamp: i64, data: &str) -> String {
  ::serde_json::json!({
    "nick": "someone",
    "features": [],
    "timestamp": timestamp,
    "data": data,
  })
  .to_string()
}

/// A `journal 0.1` journal of `records`, a record per line.
fn legacy(records: &[String]) -> String {
  let mut text = "journal 0.1\n".to_owned();
  for record in records {
    text += record;
    text.push('\n');
  }
  text
}

/// Archives `journal` as `day` under journal `root`, listed in a manifest
/// of before variants, `journal-archive 0.0`.
fn archive_legacy(root: &Path, day: &str, journal: &str) {
  let path = archive_file(root, day);
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  let compressed = ::zstd::encode_all(journal.as_bytes(), 3).unwrap();
  fs::write(&path, &compressed).unwrap();
  let manifest = archive::manifest_path(root, day);
  let mut text = fs::read_to_string(&manifest)
    .unwrap_or_else(|_| "journal-archive 0.0\n".into());
  text += &format!(
    "{day} {} {} {} {}\n",
    compressed.len(),
    journal.len(),
    journal.lines().count() - 1,
    hex(&Sha256::digest(&compressed)),
  );
  fs::write(manifest, text).unwrap();
}

/// What the archive of `day` under `root` decompresses to.
fn archived(root: &Path, day: &str) -> String {
  let compressed = fs::read(archive_file(root, day)).unwrap();
  String::from_utf8(::zstd::decode_all(compressed.as_slice()).unwrap()).unwrap()
}

/// Every file under `dir`, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
  let mut files = BTreeMap::new();
  let mut dirs = vec![dir.to_owned()];
  while let Some(dir) = dirs.pop() {
    for entry in fs::read_dir(dir).unwrap() {
      let path = entry.unwrap().path();
      if path.is_dir() {
        dirs.push(path);
      } else {
        files.insert(path.clone(), fs::read(&path).unwrap());
      }
    }
  }
  files
}

fn run(data: &Path, dry_run: bool) -> Report {
  migrate(data, dry_run, &mut |line| eprintln!("{line}")).unwrap()
}

fn schema(db: &Connection) -> i64 {
  db.pragma_query_value(None, "user_version", |row| row.get(0))
    .unwrap()
}

const RS: &str = "\u{1e}";

/// A journal of two months, and a backfill one, indexed before the
/// migration, as an index at 0.4.
fn before(dir: &TempDir) -> PathBuf {
  let data = dir.path().to_owned();
  let journal = data.join("journal");
  archive_legacy(
    &journal,
    "2026-10-31",
    &legacy(&[format!("10 a 1 frame MSG {}", message(10, "hi"))]),
  );
  archive_legacy(
    &journal,
    "2026-11-07",
    &legacy(&[
      format!("20 a 1 frame MSG {}", message(20, "one")),
      // Not JSON, so unparsed: the index has its offset.
      "30 a 1 frame MSG not json".into(),
      // An RS, so encoded again. (JSON would escape it.)
      format!("40 a 1 frame PING a{RS}b"),
      format!("50 a 1 frame MSG {}", message(50, "two")),
    ]),
  );
  archive_legacy(
    &backfill_root(&journal),
    "2026-11-07",
    &legacy(&["60 backfill-x 1 fetchrange linkers 0 60".into()]),
  );
  let mut db = open(&data.join("index.sqlite")).unwrap();
  index(&mut db, &journal).unwrap();
  db.pragma_update(None, "user_version", user_version(FROM_SCHEMA))
    .unwrap();
  data
}

#[test]
fn converts_archives_and_upgrades_the_index() {
  let dir = TempDir::new("logger-migrate");
  let data = before(&dir);
  let journal = data.join("journal");
  let db = Connection::open(data.join("index.sqlite")).unwrap();
  let messages = |db: &Connection| -> i64 {
    db.query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
      .unwrap()
  };
  let had = messages(&db);
  drop(db);

  let report = run(&data, false);
  assert_eq!(
    report,
    Report {
      converted: 3,
      manifests: 3,
      failed: vec![],
      index: Some(FROM_SCHEMA),
    }
  );
  let encoded = |text: &str| {
    use ::base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD.encode(text)
  };
  let with_rs = format!("PING a{RS}b");
  assert_eq!(
    archived(&journal, "2026-11-07"),
    format!(
      "journal dgg.0.2\n20 a 1 frame MSG {}{RS}\n30 a 1 frame MSG not json{RS}\n\
       40 a 1 frame64 {}{RS}\n50 a 1 frame MSG {}{RS}\n",
      message(20, "one"),
      encoded(&with_rs),
      message(50, "two"),
    )
  );
  // The manifests describe the new archives.
  for (root, day) in [
    (journal.clone(), "2026-10-31"),
    (journal.clone(), "2026-11-07"),
    (backfill_root(&journal), "2026-11-07"),
  ] {
    let text = fs::read_to_string(archive::manifest_path(&root, day)).unwrap();
    assert!(text.starts_with("journal-archive dgg.0.1\n"), "{text}");
    let compressed = fs::read(archive_file(&root, day)).unwrap();
    let listed = archive::read_month(&root, day).unwrap();
    let entry: &Archive = listed.iter().find(|a| a.day == day).unwrap();
    assert_eq!(entry.sha256, hex(&Sha256::digest(&compressed)));
    assert_eq!(entry.compressed, compressed.len() as u64);
    assert_eq!(entry.bytes, archived(&root, day).len() as u64);
  }

  // The index is at 0.5, its offsets into the new files.
  let db = Connection::open(data.join("index.sqlite")).unwrap();
  assert_eq!(schema(&db), user_version(SCHEMA_VERSION));
  let path = "archive/2026/11/2026-11-07.journal.zst";
  let (offset, digest): (i64, String) = db
    .query_row(
      "SELECT offset, digest FROM files WHERE path = ?1",
      [path],
      |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap();
  let new = archived(&journal, "2026-11-07");
  assert_eq!(offset as usize, new.len());
  let compressed = fs::read(archive_file(&journal, "2026-11-07")).unwrap();
  assert_eq!(digest, hex(&Sha256::digest(&compressed)));
  let unparsed: i64 = db
    .query_row(
      "SELECT offset FROM unparsed WHERE path = ?1",
      [path],
      |row| row.get(0),
    )
    .unwrap();
  assert!(new[unparsed as usize..].starts_with("30 a 1 frame MSG not json"));
  drop(db);

  // So indexing reads nothing again, and nothing's lost.
  let mut db = open(&data.join("index.sqlite")).unwrap();
  let stats = index(&mut db, &journal).unwrap();
  assert_eq!(stats.lines, 0);
  assert_eq!(messages(&db), had);
  drop(db);

  // Once done, there's nothing to do.
  let files = snapshot(&data);
  let again = run(&data, false);
  assert_eq!((again.converted, again.manifests), (0, 0));
  assert_eq!(again.index, Some(SCHEMA_VERSION));
  let mut after = snapshot(&data);
  // Only SQLite's own files may have changed.
  after.retain(|path, _| !path.to_string_lossy().contains("index.sqlite"));
  let mut files = files;
  files.retain(|path, _| !path.to_string_lossy().contains("index.sqlite"));
  assert_eq!(after, files);
}

#[test]
fn a_failing_month_is_left_as_it_was() {
  let dir = TempDir::new("logger-migrate");
  let data = before(&dir);
  let journal = data.join("journal");
  // A torn record at its end: not what compaction writes.
  archive_legacy(&journal, "2026-11-08", "journal 0.1\n70 a 1 fra");
  let november = journal.join("archive/2026/11");
  let was = snapshot(&november);

  let report = run(&data, false);
  assert_eq!(report.converted, 2);
  assert_eq!(report.failed.len(), 1);
  assert!(
    report.failed[0].starts_with("archive/2026/11: 2026-11-08: a torn"),
    "{:?}",
    report.failed
  );
  assert_eq!(snapshot(&november), was);
  // The other months were migrated, but the index stays at 0.4 until all
  // are.
  assert!(archived(&journal, "2026-10-31").starts_with("journal dgg.0.2\n"));
  let db = Connection::open(data.join("index.sqlite")).unwrap();
  assert_eq!(schema(&db), user_version(FROM_SCHEMA));
}

#[test]
fn a_dry_run_changes_nothing() {
  let dir = TempDir::new("logger-migrate");
  let data = before(&dir);
  // But for the files SQLite keeps beside a database it opens.
  let files = || {
    let mut files = snapshot(&data);
    files.retain(|path, _| !path.to_string_lossy().contains("index.sqlite-"));
    files
  };
  let was = files();
  let report = run(&data, true);
  assert_eq!((report.converted, report.failed.len()), (3, 0));
  assert_eq!(files(), was);
}

#[test]
fn waits_for_a_supervisors_locks() {
  let dir = TempDir::new("logger-migrate");
  let data = before(&dir);
  let lock = File::create(data.join("compact.lock")).unwrap();
  lock.lock().unwrap();
  let (done, finished) = mpsc::channel();
  let at = data.clone();
  let migrating = thread::spawn(move || {
    let report = run(&at, false);
    done.send(()).unwrap();
    report
  });
  assert!(finished.recv_timeout(Duration::from_millis(1500)).is_err());
  assert!(
    archived(&data.join("journal"), "2026-10-31").starts_with("journal 0.1")
  );
  drop(lock);
  assert_eq!(migrating.join().unwrap().converted, 3);
}
