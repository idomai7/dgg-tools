use {
  ::common::{temp_dir::TempDir, time::now_ms},
  ::logger::{
    compact::{closed_days, compact_day, compact_open_day, open_days},
    control,
    service::Root,
  },
  ::logger_journal::{
    Reader, Writer, archive,
    layout::{archive_file, backfill_root, live_day_dir, lock_file},
    record::v0::Body,
  },
  ::sha2::{Digest, Sha256},
  ::std::{
    fs,
    time::{Duration, SystemTime},
  },
};

/// Writes a live file for 1970-01-01 verbatim.
fn live(dir: &TempDir, name: &str, text: &str) {
  let dir = live_day_dir(dir.path(), DAY);
  fs::create_dir_all(&dir).unwrap();
  fs::write(dir.join(format!("{name}.journal")), text).unwrap();
}

/// A `journal dgg.0.3` of `records`, each ending at RS LF.
fn journal(records: &[&str]) -> String {
  let mut text = "journal dgg.0.3\u{1e}\n".to_owned();
  for record in records {
    text += record;
    text += "\u{1e}\n";
  }
  text
}

fn archived(dir: &TempDir) -> String {
  let file = fs::File::open(archive_file(dir.path(), DAY)).unwrap();
  String::from_utf8(::zstd::decode_all(file).unwrap()).unwrap()
}

const DAY: &str = "1970-01-01";

#[test]
fn finds_closed_days() {
  let root = TempDir::new("logger-compact");
  for day in ["1970-01-01", "1970-01-02", "not-a-day"] {
    fs::create_dir_all(live_day_dir(root.path(), day)).unwrap();
  }
  let hour = Duration::from_secs(3600);
  // Half an hour into 01-03: 01-01 ended over an hour ago, 01-02 didn't.
  let now = 2 * 86_400_000 + 1_800_000;
  assert_eq!(
    closed_days(&Root::at(root.path()), now, hour).unwrap(),
    ["1970-01-01"]
  );
  assert_eq!(
    closed_days(&Root::at(root.path()), now, Duration::ZERO)
      .unwrap()
      .len(),
    2
  );
  assert_eq!(
    open_days(&Root::at(root.path()), now, hour).unwrap(),
    ["1970-01-02"]
  );
}

#[test]
fn compacts_what_captures_finished_on_an_open_day() {
  let root = TempDir::new("logger-compact");
  let control = control::dir(root.path());
  fs::create_dir_all(&control).unwrap();
  for (n, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
    live(
      &root,
      name,
      &journal(&[&format!("{n} {name} 1 frame MSG {n}")]),
    );
  }
  // a's capture holds its live lock.
  let a = Writer::capture(root.path(), "a").unwrap();
  // c's, from before the lock, has a fresh status; d's left a stale one.
  fs::write(control::path(&control, "c"), "").unwrap();
  let stale = SystemTime::now() - Duration::from_secs(60);
  let d = fs::File::create(control::path(&control, "d")).unwrap();
  d.set_modified(stale).unwrap();
  drop(d);

  let now = now_ms();
  let done = compact_open_day(&Root::at(root.path()), DAY, 3, now)
    .unwrap()
    .unwrap();
  assert_eq!((done.files, done.capturing), (2, 2));
  assert_eq!(
    archived(&root),
    journal(&["1 b 1 frame MSG 1", "3 d 1 frame MSG 3"])
  );
  let dir = live_day_dir(root.path(), DAY);
  assert!(dir.join("a.journal").exists() && dir.join("c.journal").exists());

  // Once they've stopped, the rest join the archive.
  drop(a);
  fs::remove_file(control::path(&control, "c")).unwrap();
  let done = compact_open_day(&Root::at(root.path()), DAY, 3, now)
    .unwrap()
    .unwrap();
  assert_eq!((done.files, done.capturing, done.remerged), (2, 0, true));
  assert_eq!(done.archive.records, 4);
  assert!(!dir.exists());
}

#[test]
fn a_capture_holds_back_only_open_days() {
  let root = TempDir::new("logger-compact");
  live(&root, "a", &journal(&["1 a 1 frame MSG 1"]));
  let _a = Writer::capture(root.path(), "a").unwrap();
  // Once the day is closed, a capture still running writes another's.
  let done = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert_eq!((done.files, done.capturing), (1, 0));
}

#[test]
fn merges_a_day_into_a_verified_archive() {
  let root = TempDir::new("logger-compact");
  let a = journal(&["10 a 1 frame MSG 1", "30 a 1 frame MSG 3"]);
  live(&root, "a", &a);
  // b interleaves with a, and ends with a record torn by a crash.
  let b = journal(&["20 b 1 frame MSG 2", "30 b 1 open u"]) + "40 b 1 fr";
  live(&root, "b", &b);
  // A copy of a's file: every record an exact duplicate.
  live(&root, "c", &a);

  let done = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert_eq!(
    (
      done.files,
      done.grown,
      done.remerged,
      done.duplicates,
      done.torn
    ),
    (3, 0, false, 2, 1)
  );
  // Sorted by receive time; equal times keep their order (a before b).
  assert_eq!(
    archived(&root),
    journal(&[
      "10 a 1 frame MSG 1",
      "20 b 1 frame MSG 2",
      "30 a 1 frame MSG 3",
      "30 b 1 open u",
    ])
  );
  // The manifest describes it exactly.
  let compressed = fs::read(archive_file(root.path(), DAY)).unwrap();
  let digest: String = Sha256::digest(&compressed)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
  let recorded = archive::read_month(root.path(), DAY).unwrap();
  assert_eq!(recorded.len(), 1);
  let entry = &recorded[0];
  assert_eq!(entry.compressed, compressed.len() as u64);
  assert_eq!(entry.bytes, archived(&root).len() as u64);
  assert_eq!((entry.records, entry.sha256.as_str()), (4, digest.as_str()));
  // The live files and their day directory are gone; the archive reads.
  assert!(!live_day_dir(root.path(), DAY).exists());
  let path = archive_file(root.path(), DAY);
  assert_eq!(Reader::open(&path, 0).unwrap().count(), 4);
  assert!(!path.with_extension("zst.tmp").exists());
}

#[test]
fn merges_late_files_into_an_existing_archive() {
  let root = TempDir::new("logger-compact");
  let a = journal(&["10 a 1 frame MSG 1", "30 a 1 frame MSG 3"]);
  live(&root, "a", &a);
  compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  // A late sync brings another instance's file for the same day, and a
  // second copy of a's.
  live(&root, "b", &journal(&["20 b 1 frame MSG 2"]));
  live(&root, "a", &a);
  let done = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert!(done.remerged);
  assert_eq!((done.files, done.duplicates), (2, 2));
  assert_eq!(
    archived(&root),
    journal(&[
      "10 a 1 frame MSG 1",
      "20 b 1 frame MSG 2",
      "30 a 1 frame MSG 3",
    ])
  );
  assert_eq!(archive::read_month(root.path(), DAY).unwrap()[0].records, 3);
  // Nothing left to do.
  assert_eq!(compact_day(&Root::at(root.path()), DAY, 3).unwrap(), None);
}

#[test]
fn refuses_unreadable_versions() {
  for other in [
    // Within major 0 another minor may mean anything.
    "journal dgg.0.7\u{1e}\n10 a 1 shiny new kind\u{1e}\n",
    // Older versions are no longer read: every host was migrated.
    "journal dgg.0.2\n10 b 1 frame MSG 2\u{1e}\n",
    "journal 0.1\n10 b 1 frame MSG 2\n",
  ] {
    let root = TempDir::new("logger-compact");
    live(&root, "a", &journal(&["10 a 1 frame MSG 1"]));
    live(&root, "b", other);
    assert!(
      compact_day(&Root::at(root.path()), DAY, 3).is_err(),
      "{other:?}"
    );
    assert!(!archive_file(root.path(), DAY).exists());
  }
}

#[test]
fn leaves_unreadable_days_alone() {
  let root = TempDir::new("logger-compact");
  live(&root, "a", &journal(&["10 a 1 frame MSG 1"]));
  live(
    &root,
    "b",
    "journal dgg.2.0\u{1e}\n20 b 1 something\u{1e}\n",
  );
  assert!(compact_day(&Root::at(root.path()), DAY, 3).is_err());
  assert!(!archive_file(root.path(), DAY).exists());
  let left = fs::read_dir(live_day_dir(root.path(), DAY))
    .unwrap()
    .count();
  assert_eq!(left, 2);
}

#[test]
fn keeps_records_repeated_within_one_file() {
  let root = TempDir::new("logger-compact");
  // dgg really does send the same frame twice in one millisecond.
  let twice = journal(&["10 a 1 frame UPDATEUSER {}"; 2]);
  live(&root, "a", &twice);
  let done = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert_eq!(done.duplicates, 0);
  assert_eq!(archived(&root), twice);
  // Merging the same file in again, twice, doesn't add more copies.
  live(&root, "a", &twice);
  live(&root, "b", &twice);
  let again = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert_eq!(again.duplicates, 4);
  assert_eq!(archived(&root), twice);
}

#[test]
fn leaves_the_files_of_a_running_backfill() {
  let root = TempDir::new("logger-compact");
  let backfill = backfill_root(root.path());
  let dir = live_day_dir(&backfill, DAY);
  fs::create_dir_all(&dir).unwrap();
  fs::write(
    dir.join("backfill-done.journal"),
    journal(&["5 backfill-done 1 fetchrange x 0 5"]),
  )
  .unwrap();
  // The running one also has a file with another header than it writes, so
  // it writes to the day's next file, which is just as busy.
  fs::write(
    dir.join("backfill-running.journal"),
    "journal dgg.0.3 ; note=x\u{1e}\n",
  )
  .unwrap();
  let mut running = Writer::backfill(root.path(), "backfill-running").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 5,
    to: 10,
  };
  running.write_on(5, 10, 1, range).unwrap();
  running.write_on(5, 11, 1, Body::Frame("x".into())).unwrap();
  running.sync().unwrap();
  assert!(dir.join("backfill-running.2.journal").exists());

  // Only the finished run's file is compacted.
  let done = compact_day(&Root::at(&backfill), DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.busy, done.archive.records), (1, 2, 1));
  assert!(dir.join("backfill-running.journal").exists());
  assert!(dir.join("backfill-running.2.journal").exists());

  // Once the run is over, its file is merged in, and its lock is gone.
  drop(running);
  assert!(!lock_file(&backfill, "backfill-running").exists());
  let done = compact_day(&Root::at(&backfill), DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.busy, done.remerged), (2, 0, true));
  assert_eq!(done.archive.records, 3);
  assert!(!dir.exists());
}

/// A Kick journal at `version` (`0.0` or `0.1`) of `records`, its header
/// naming `endpoint` and channel `c`.
fn kick_journal(version: &str, endpoint: &str, records: &[&str]) -> String {
  let mut text =
    format!("journal kick.{version} ; endpoint={endpoint} channel=c\u{1e}\n");
  for record in records {
    text += record;
    text += "\u{1e}\n";
  }
  text
}

#[test]
fn merges_a_kick_channels_day() {
  let root = TempDir::new("logger-compact");
  live(
    &root,
    "a",
    &kick_journal(
      "0.0",
      "wss://one",
      &["10 a 1 open wss://one", "30 a 1 frame {}"],
    ),
  );
  live(
    &root,
    "a.2",
    &kick_journal("0.1", "wss://two", &["20 a 2 open wss://two"]),
  );

  let done = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert_eq!((done.files, done.archive.records), (2, 3));
  // In the newest version, keeping only what both headers say.
  assert_eq!(
    archived(&root),
    "journal kick.0.1 ; channel=c\u{1e}\n\
     10 a 1 open wss://one\u{1e}\n\
     20 a 2 open wss://two\u{1e}\n\
     30 a 1 frame {}\u{1e}\n"
  );
  let manifest = fs::read_to_string(archive::manifest_path(root.path(), DAY));
  assert!(manifest.unwrap().starts_with("journal-archive kick.0.0"));
  let reader = Reader::open(&archive_file(root.path(), DAY), 0).unwrap();
  assert_eq!(reader.count(), 3);

  // A day with one endpoint keeps it, merged into the existing archive.
  live(
    &root,
    "b",
    &kick_journal("0.1", "wss://one", &["40 b 1 open wss://one"]),
  );
  let done = compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert!(done.remerged);
  assert!(archived(&root).starts_with("journal kick.0.1 ; channel=c\u{1e}\n"));
}

#[test]
fn refuses_a_day_of_mixed_variants() {
  let root = TempDir::new("logger-compact");
  live(&root, "a", &journal(&["10 a 1 frame MSG 1"]));
  live(
    &root,
    "b",
    &kick_journal("0.1", "wss://one", &["20 b 1 frame {}"]),
  );
  assert!(compact_day(&Root::at(root.path()), DAY, 3).is_err());
  assert!(!archive_file(root.path(), DAY).exists());
}

#[test]
fn keeps_a_single_endpoint() {
  let root = TempDir::new("logger-compact");
  live(
    &root,
    "a",
    &kick_journal("0.1", "wss://one", &["10 a 1 frame {}"]),
  );
  live(
    &root,
    "b",
    &kick_journal("0.1", "wss://one", &["20 b 1 frame {}"]),
  );
  compact_day(&Root::at(root.path()), DAY, 3)
    .unwrap()
    .unwrap();
  assert!(
    archived(&root)
      .starts_with("journal kick.0.1 ; endpoint=wss://one channel=c\u{1e}\n")
  );
}

/// A root's live files in its legacy directory (a capture from before
/// dgg's journals moved into dgg/ wrote them) are its day's too: merged
/// into its archive, and deleted from there.
#[test]
fn merges_live_files_where_they_were() {
  let (root, legacy) = (
    TempDir::new("logger-compact"),
    TempDir::new("logger-compact"),
  );
  live(&root, "a", &journal(&["10 a 1 frame MSG 1"]));
  live(&legacy, "b", &journal(&["20 b 1 frame MSG 2"]));
  let both = Root {
    legacy: Some(legacy.to_path_buf()),
    ..Root::at(root.path())
  };
  let hour = Duration::from_secs(3600);
  assert_eq!(closed_days(&both, 86_400_000 * 2, hour).unwrap(), [DAY]);
  let done = compact_day(&both, DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.archive.records), (2, 2));
  assert_eq!(
    archived(&root),
    journal(&["10 a 1 frame MSG 1", "20 b 1 frame MSG 2"])
  );
  assert!(!live_day_dir(legacy.path(), DAY).exists());
  assert!(!archive_file(legacy.path(), DAY).exists());
}
