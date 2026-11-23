use {
  ::common::temp_dir::TempDir,
  ::logger::compact::{closed_days, compact_day},
  ::logger_journal::{
    Reader, Writer, archive,
    layout::{archive_file, backfill_root, live_day_dir, lock_file},
    record::v0::Body,
  },
  ::sha2::{Digest, Sha256},
  ::std::{fs, time::Duration},
};

/// Writes a live file for 1970-01-01 verbatim.
fn live(dir: &TempDir, name: &str, text: &str) {
  let dir = live_day_dir(dir.path(), DAY);
  fs::create_dir_all(&dir).unwrap();
  fs::write(dir.join(format!("{name}.journal")), text).unwrap();
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
  assert_eq!(closed_days(root.path(), now, hour).unwrap(), ["1970-01-01"]);
  assert_eq!(
    closed_days(root.path(), now, Duration::ZERO).unwrap().len(),
    2
  );
}

#[test]
fn merges_a_day_into_a_verified_archive() {
  let root = TempDir::new("logger-compact");
  live(
    &root,
    "a",
    "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n",
  );
  // b interleaves with a, and ends with a record torn by a crash.
  live(
    &root,
    "b",
    "journal 0.1\n20 b 1 frame MSG 2\n30 b 1 open u\n40 b 1 fr",
  );
  // A copy of a's file: every record an exact duplicate.
  live(
    &root,
    "c",
    "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n",
  );

  let done = compact_day(root.path(), DAY, 3).unwrap().unwrap();
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
  // Sorted by receive time; equal times keep their order (a before b). In
  // the newest version, each record ending at RS LF.
  assert_eq!(
    archived(&root),
    "journal dgg.0.3\u{1e}\n10 a 1 frame MSG 1\u{1e}\n20 b 1 frame MSG 2\u{1e}\n\
     30 a 1 frame MSG 3\u{1e}\n30 b 1 open u\u{1e}\n"
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
  live(
    &root,
    "a",
    "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n",
  );
  compact_day(root.path(), DAY, 3).unwrap().unwrap();
  // A late sync brings another instance's file for the same day, and a
  // second copy of a's.
  live(&root, "b", "journal 0.1\n20 b 1 frame MSG 2\n");
  live(
    &root,
    "a",
    "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n",
  );
  let done = compact_day(root.path(), DAY, 3).unwrap().unwrap();
  assert!(done.remerged);
  assert_eq!((done.files, done.duplicates), (2, 2));
  assert_eq!(
    archived(&root),
    "journal dgg.0.3\u{1e}\n10 a 1 frame MSG 1\u{1e}\n20 b 1 frame MSG 2\u{1e}\n\
     30 a 1 frame MSG 3\u{1e}\n"
  );
  assert_eq!(archive::read_month(root.path(), DAY).unwrap()[0].records, 3);
  // Nothing left to do.
  assert_eq!(compact_day(root.path(), DAY, 3).unwrap(), None);
}

#[test]
fn refuses_unreadable_versions() {
  let root = TempDir::new("logger-compact");
  live(&root, "a", "journal 0.1\n10 a 1 frame MSG 1\n");
  // Within major 0 another minor may mean anything.
  live(&root, "b", "journal 0.7\n10 a 1 shiny new kind\n");
  assert!(compact_day(root.path(), DAY, 3).is_err());
  assert!(!archive_file(root.path(), DAY).exists());
}

#[test]
fn leaves_unreadable_days_alone() {
  let root = TempDir::new("logger-compact");
  live(&root, "a", "journal 0.1\n10 a 1 frame MSG 1\n");
  live(&root, "b", "journal 2.0\n20 b 1 something\n");
  assert!(compact_day(root.path(), DAY, 3).is_err());
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
  let twice =
    "journal 0.1\n10 a 1 frame UPDATEUSER {}\n10 a 1 frame UPDATEUSER {}\n";
  let archive = "journal dgg.0.3\u{1e}\n10 a 1 frame UPDATEUSER {}\u{1e}\n\
                 10 a 1 frame UPDATEUSER {}\u{1e}\n";
  live(&root, "a", twice);
  let done = compact_day(root.path(), DAY, 3).unwrap().unwrap();
  assert_eq!(done.duplicates, 0);
  assert_eq!(archived(&root), archive);
  // Merging the same file in again doesn't add a third and fourth copy, from
  // the old version or the new.
  live(&root, "a", twice);
  live(&root, "b", archive);
  let again = compact_day(root.path(), DAY, 3).unwrap().unwrap();
  assert_eq!(again.duplicates, 4);
  assert_eq!(archived(&root), archive);
}

#[test]
fn converts_records_of_older_versions() {
  let root = TempDir::new("logger-compact");
  // 0.1: an LF in data went base64, an RS didn't.
  live(
    &root,
    "a",
    "journal 0.1\n10 a 1 frame64 QQpC\n20 a 1 frame A\u{1e}B\n",
  );
  live(
    &root,
    "b",
    "journal dgg.0.3\u{1e}\n15 b 1 frame C\nD\u{1e}\n",
  );
  // 0.2: only its header ended at LF.
  live(&root, "c", "journal dgg.0.2\n17 c 1 frame E\u{1e}\n");
  compact_day(root.path(), DAY, 3).unwrap().unwrap();
  // Copied byte for byte, but for the RS, now base64.
  assert_eq!(
    archived(&root),
    "journal dgg.0.3\u{1e}\n10 a 1 frame64 QQpC\u{1e}\n15 b 1 frame C\nD\u{1e}\n\
     17 c 1 frame E\u{1e}\n20 a 1 frame64 QR5C\u{1e}\n"
  );

  // A record that doesn't decode can't keep its RS: the day is refused.
  let root = TempDir::new("logger-compact");
  live(&root, "a", "journal 0.1\n10 a 1 frame64 !\u{1e}\n");
  let error = compact_day(root.path(), DAY, 3).unwrap_err();
  assert!(error.to_string().contains("holds an RS"), "{error}");
  assert!(!archive_file(root.path(), DAY).exists());
}

#[test]
fn leaves_the_files_of_a_running_backfill() {
  let root = TempDir::new("logger-compact");
  let backfill = backfill_root(root.path());
  let dir = live_day_dir(&backfill, DAY);
  fs::create_dir_all(&dir).unwrap();
  fs::write(
    dir.join("backfill-done.journal"),
    "journal 0.1
5 backfill-done 1 fetchrange x 0 5
",
  )
  .unwrap();
  // The running one also has a file from before the format changed, so it
  // writes to the day's next file, which is just as busy.
  fs::write(dir.join("backfill-running.journal"), "journal 0.1\n").unwrap();
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
  let done = compact_day(&backfill, DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.busy, done.archive.records), (1, 2, 1));
  assert!(dir.join("backfill-running.journal").exists());
  assert!(dir.join("backfill-running.2.journal").exists());

  // Once the run is over, its file is merged in, and its lock is gone.
  drop(running);
  assert!(!lock_file(&backfill, "backfill-running").exists());
  let done = compact_day(&backfill, DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.busy, done.remerged), (2, 0, true));
  assert_eq!(done.archive.records, 3);
  assert!(!dir.exists());
}
