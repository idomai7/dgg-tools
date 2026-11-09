use {
  ::journal::{
    Reader, Writer,
    archive::{self},
    layout::{archive_file, backfill_root, live_day_dir, lock_file},
    record::v0::Body,
  },
  ::logger::compact::{closed_days, compact_day},
  ::sha2::{Digest, Sha256},
  ::std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
  },
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("logger-compact-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }

  /// Writes a live file for 1970-01-01 verbatim.
  fn live(&self, name: &str, text: &str) {
    let dir = live_day_dir(&self.0, DAY);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.journal")), text).unwrap();
  }

  fn archived(&self) -> String {
    let file = fs::File::open(archive_file(&self.0, DAY)).unwrap();
    String::from_utf8(::zstd::decode_all(file).unwrap()).unwrap()
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

const DAY: &str = "1970-01-01";

#[test]
fn finds_closed_days() {
  let root = TempRoot::new();
  for day in ["1970-01-01", "1970-01-02", "not-a-day"] {
    fs::create_dir_all(live_day_dir(&root.0, day)).unwrap();
  }
  let hour = Duration::from_secs(3600);
  // Half an hour into 01-03: 01-01 ended over an hour ago, 01-02 didn't.
  let now = 2 * 86_400_000 + 1_800_000;
  assert_eq!(closed_days(&root.0, now, hour).unwrap(), ["1970-01-01"]);
  assert_eq!(closed_days(&root.0, now, Duration::ZERO).unwrap().len(), 2);
}

#[test]
fn merges_a_day_into_a_verified_archive() {
  let root = TempRoot::new();
  root.live("a", "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n");
  // b interleaves with a, and ends with a line torn by a crash.
  root.live(
    "b",
    "journal 0.1\n20 b 1 frame MSG 2\n30 b 1 open u\n40 b 1 fr",
  );
  // A copy of a's file: every line an exact duplicate.
  root.live("c", "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n");

  let done = compact_day(&root.0, DAY, 3).unwrap().unwrap();
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
    root.archived(),
    "journal 0.1\n10 a 1 frame MSG 1\n20 b 1 frame MSG 2\n\
     30 a 1 frame MSG 3\n30 b 1 open u\n"
  );
  // The manifest describes it exactly.
  let compressed = fs::read(archive_file(&root.0, DAY)).unwrap();
  let digest: String = Sha256::digest(&compressed)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
  let recorded = archive::read_month(&root.0, DAY).unwrap();
  assert_eq!(recorded.len(), 1);
  let entry = &recorded[0];
  assert_eq!(entry.compressed, compressed.len() as u64);
  assert_eq!(entry.bytes, root.archived().len() as u64);
  assert_eq!((entry.lines, entry.sha256.as_str()), (4, digest.as_str()));
  // The live files and their day directory are gone; the archive reads.
  assert!(!live_day_dir(&root.0, DAY).exists());
  let path = archive_file(&root.0, DAY);
  assert_eq!(Reader::open(&path, 0).unwrap().count(), 4);
  assert!(!path.with_extension("zst.tmp").exists());
}

#[test]
fn merges_late_files_into_an_existing_archive() {
  let root = TempRoot::new();
  root.live("a", "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n");
  compact_day(&root.0, DAY, 3).unwrap().unwrap();
  // A late sync brings another instance's file for the same day, and a
  // second copy of a's.
  root.live("b", "journal 0.1\n20 b 1 frame MSG 2\n");
  root.live("a", "journal 0.1\n10 a 1 frame MSG 1\n30 a 1 frame MSG 3\n");
  let done = compact_day(&root.0, DAY, 3).unwrap().unwrap();
  assert!(done.remerged);
  assert_eq!((done.files, done.duplicates), (2, 2));
  assert_eq!(
    root.archived(),
    "journal 0.1\n10 a 1 frame MSG 1\n20 b 1 frame MSG 2\n30 a 1 frame MSG 3\n"
  );
  assert_eq!(archive::read_month(&root.0, DAY).unwrap()[0].lines, 3);
  // Nothing left to do.
  assert_eq!(compact_day(&root.0, DAY, 3).unwrap(), None);
}

#[test]
fn refuses_unreadable_versions() {
  let root = TempRoot::new();
  root.live("a", "journal 0.1\n10 a 1 frame MSG 1\n");
  // Within major 0 another minor may mean anything.
  root.live("b", "journal 0.7\n10 a 1 shiny new kind\n");
  assert!(compact_day(&root.0, DAY, 3).is_err());
  assert!(!archive_file(&root.0, DAY).exists());
}

#[test]
fn leaves_unreadable_days_alone() {
  let root = TempRoot::new();
  root.live("a", "journal 0.1\n10 a 1 frame MSG 1\n");
  root.live("b", "journal 2.0\n20 b 1 something\n");
  assert!(compact_day(&root.0, DAY, 3).is_err());
  assert!(!archive_file(&root.0, DAY).exists());
  let left = fs::read_dir(live_day_dir(&root.0, DAY)).unwrap().count();
  assert_eq!(left, 2);
}

#[test]
fn keeps_lines_repeated_within_one_file() {
  let root = TempRoot::new();
  // dgg really does send the same frame twice in one millisecond.
  let twice =
    "journal 0.1\n10 a 1 frame UPDATEUSER {}\n10 a 1 frame UPDATEUSER {}\n";
  root.live("a", twice);
  let done = compact_day(&root.0, DAY, 3).unwrap().unwrap();
  assert_eq!(done.duplicates, 0);
  assert_eq!(root.archived(), twice);
  // Merging the same file in again doesn't add a third and fourth copy.
  root.live("a", twice);
  let again = compact_day(&root.0, DAY, 3).unwrap().unwrap();
  assert_eq!(again.duplicates, 2);
  assert_eq!(root.archived(), twice);
}

#[test]
fn leaves_the_files_of_a_running_backfill() {
  let root = TempRoot::new();
  let backfill = backfill_root(&root.0);
  let dir = live_day_dir(&backfill, DAY);
  fs::create_dir_all(&dir).unwrap();
  fs::write(
    dir.join("backfill-done.journal"),
    "journal 0.1
5 backfill-done 1 fetchrange x 0 5
",
  )
  .unwrap();
  let mut running = Writer::backfill(&root.0, "backfill-running").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 5,
    to: 10,
  };
  running.write_on(5, 10, 1, range).unwrap();
  running.sync().unwrap();

  // Only the finished run's file is compacted.
  let done = compact_day(&backfill, DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.busy, done.archive.lines), (1, 1, 1));
  assert!(dir.join("backfill-running.journal").exists());

  // Once the run is over, its file is merged in, and its lock is gone.
  drop(running);
  assert!(!lock_file(&backfill, "backfill-running").exists());
  let done = compact_day(&backfill, DAY, 3).unwrap().unwrap();
  assert_eq!((done.files, done.busy, done.remerged), (1, 0, true));
  assert_eq!(done.archive.lines, 2);
  assert!(!dir.exists());
}
