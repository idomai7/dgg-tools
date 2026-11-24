//! Migrating: dgg's journals move into `journal/dgg/` while a capture from
//! before may still be writing, and it checks the version of each kind of
//! file and each index's schema, under a supervisor's locks.

use {
  ::common::{temp_dir::TempDir, time::now_ms},
  ::logger::{
    BUILD, control,
    dgg::{
      Dgg,
      index::{FILE, LEGACY_FILE, SCHEMA_VERSION, open},
    },
    index::user_version,
    migrate::{Report, migrate},
    service::Service,
    taken,
  },
  ::logger_journal::{
    Header, Variant, Version,
    record::{kick, v0},
  },
  ::std::{
    fs::{self, File},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
  },
};

fn run(data: &Path, dry_run: bool) -> Report {
  migrate(data, dry_run, &mut |_, line| eprintln!("{line}")).unwrap()
}

#[test]
fn nothing_to_migrate() {
  let dir = TempDir::new("logger-migrate");
  assert_eq!(run(dir.path(), false), Report::default());
  drop(open(&dir.path().join(FILE)).unwrap());
  for dry_run in [false, true] {
    let report = run(dir.path(), dry_run);
    assert_eq!(report.failed, Vec::<String>::new());
    assert_eq!(report.index, Some(SCHEMA_VERSION));
  }
}

#[test]
fn an_index_at_another_schema_fails() {
  let dir = TempDir::new("logger-migrate");
  let db = open(&dir.path().join(FILE)).unwrap();
  let old = Version { major: 0, minor: 5 };
  db.pragma_update(None, "user_version", user_version(old))
    .unwrap();
  drop(db);
  let report = run(dir.path(), false);
  assert_eq!(report.index, Some(old));
  assert_eq!(report.failed.len(), 1);
}

#[test]
fn waits_for_a_supervisors_locks() {
  let dir = TempDir::new("logger-migrate");
  let data = dir.to_path_buf();
  let lock = File::create(data.join("compact.lock")).unwrap();
  lock.lock().unwrap();
  let (done, finished) = mpsc::channel();
  let migrating = thread::spawn(move || {
    run(&data, false);
    done.send(()).unwrap();
  });
  assert!(finished.recv_timeout(Duration::from_millis(1500)).is_err());
  // A dry run takes no locks.
  run(dir.path(), true);
  drop(lock);
  migrating.join().unwrap();
}

/// Writes `text` at `path`, making its directory.
fn put(path: &Path, text: &str) {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, text).unwrap();
}

#[test]
fn says_each_kind_of_file() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let dgg_dir = Dgg::dir(&journal);
  let dgg = Header::new(Variant::Dgg, v0::VERSION).text();
  let old = Header::new(Variant::Dgg, Version { major: 0, minor: 2 }).text();
  put(&dgg_dir.join("live/2026-11-24/a.journal"), &dgg);
  put(&dgg_dir.join("live/2026-11-24/b.journal"), &old);
  put(&dgg_dir.join("backfill/live/2026-11-20/c.journal"), &dgg);
  let kick = Header::new(Variant::Kick, kick::v0::VERSION).text();
  put(
    &journal.join("kick/someone/live/2026-11-24/d.journal"),
    &kick,
  );
  let now = now_ms();
  let status = |instance: &str, build: &str| control::Status {
    instance: instance.into(),
    started: now - 1000,
    heartbeat: now,
    healthy_since: Some(now - 500),
    build: Some(build.into()),
  };
  let control = control::dir(&dgg_dir);
  control::write(&control, &status("old", "0.0.1-abc")).unwrap();
  control::write(&control, &status("new", BUILD)).unwrap();

  let mut said = Vec::new();
  let report =
    migrate(dir.path(), false, &mut |_, line| said.push(line.to_owned()))
      .unwrap();
  let said = said.join(
    "
",
  );
  assert!(said.starts_with(&format!("build {BUILD}:")), "{said}");
  for line in [
    "dgg: 2 live files, journal dgg.0.3; 1 at another version",
    "dgg: live/2026-11-24/b.journal is journal dgg.0.2",
    "dgg: 2 capture statuses, capture-status dgg.0.2",
    "dgg backfill: 1 live file, journal dgg.0.3",
    "kick someone: 1 live file, journal kick.0.1",
    "dgg's index: none yet",
    "Kick's index: none yet",
  ] {
    assert!(
      said.contains(line),
      "{line:?} not in:
{said}"
    );
  }
  assert_eq!(
    report.unreadable,
    [dgg_dir.join("live/2026-11-24/b.journal")]
  );
  assert_eq!(report.failed, Vec::<String>::new());
}

/// A capture status written `age` ms ago.
fn status_at(instance: &str, age: i64) -> control::Status {
  let now = now_ms();
  control::Status {
    instance: instance.into(),
    started: now - 60_000,
    heartbeat: now - age,
    healthy_since: Some(now - 50_000),
    build: None,
  }
}

#[test]
fn moves_dggs_journals_into_their_own_directory() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let dgg_dir = Dgg::dir(&journal);
  let header = Header::new(Variant::Dgg, v0::VERSION).text();
  // A capture from before still writing, and one that's gone.
  let old_control = control::dir(&journal);
  control::write(&old_control, &status_at("writing", 0)).unwrap();
  control::write(&old_control, &status_at("gone", 60_000)).unwrap();
  put(&journal.join("live/2026-11-24/writing.journal"), &header);
  put(&journal.join("live/2026-11-24/gone.journal"), &header);
  put(&journal.join("live/2026-11-23/gone.2.journal"), &header);
  put(&journal.join("archive/2026/11/manifest"), "m");
  put(&journal.join("archive/2026/11/2026-11-01.journal.zst"), "z");
  let sha = "ab".repeat(32);
  taken::add(&journal, Variant::Dgg, "2026-11-01", &sha).unwrap();
  // A backfill run that finished, leaving its lock.
  put(
    &journal.join("backfill/live/2026-11-20/backfill-x.journal"),
    &header,
  );
  put(&journal.join("backfill/control/backfill-x.lock"), "");
  // Kick's are where they belong already.
  let kick = journal.join("kick/c/live/2026-11-24/k.journal");
  put(&kick, "k");
  // The index, as it was named, open (so with its WAL files) meanwhile.
  let index = open(&dir.path().join(LEGACY_FILE)).unwrap();
  index.execute_batch("CREATE TABLE marker (x)").unwrap();
  assert!(dir.path().join(format!("{LEGACY_FILE}-wal")).exists());
  drop(index);

  let report = run(dir.path(), false);
  assert_eq!(report.failed, Vec::<String>::new());
  assert_eq!(report.index, Some(SCHEMA_VERSION));
  assert_eq!(report.moved, 6);
  assert_eq!(
    report.left,
    [journal.join("live/2026-11-24/writing.journal")]
  );
  for moved in [
    "live/2026-11-24/gone.journal",
    "live/2026-11-23/gone.2.journal",
    "archive/2026/11/manifest",
    "archive/2026/11/2026-11-01.journal.zst",
    "backfill/live/2026-11-20/backfill-x.journal",
  ] {
    assert!(dgg_dir.join(moved).exists(), "{moved}");
    assert!(!journal.join(moved).exists(), "{moved}");
  }
  assert_eq!(
    taken::read(&dgg_dir, Variant::Dgg).unwrap(),
    [("2026-11-01".to_owned(), sha)]
  );
  assert!(!journal.join(taken::PATH).exists());
  assert!(!journal.join("archive").exists());
  assert!(!journal.join("backfill").exists());
  assert!(kick.exists());
  let index = open(&dir.path().join(FILE)).unwrap();
  index.execute_batch("SELECT x FROM marker").unwrap();
  drop(index);
  assert!(!dir.path().join(LEGACY_FILE).exists());
  // Only the status of the capture still writing is left.
  let left: Vec<_> = control::read_all(&old_control)
    .unwrap()
    .into_iter()
    .map(|status| status.instance)
    .collect();
  assert_eq!(left, ["writing"]);

  // Once it's handed off, the next start moves the rest.
  control::remove(&old_control, "writing").unwrap();
  let report = run(dir.path(), false);
  assert_eq!((report.moved, report.left.len()), (1, 0));
  assert!(dgg_dir.join("live/2026-11-24/writing.journal").exists());
  let mut names: Vec<_> = fs::read_dir(&journal)
    .unwrap()
    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
    .collect();
  names.sort();
  assert_eq!(names, ["dgg", "kick"]);
}

#[test]
fn a_copied_status_is_not_a_capture_writing() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let header = Header::new(Variant::Dgg, v0::VERSION).text();
  // A capture from this build writes its status in both.
  let status = status_at("new", 0);
  control::write(&control::dir(&journal), &status).unwrap();
  control::write(&control::dir(&Dgg::dir(&journal)), &status).unwrap();
  put(&journal.join("live/2026-11-24/new.journal"), &header);
  let report = run(dir.path(), false);
  assert_eq!((report.moved, report.left.len()), (1, 0));
  assert!(!control::dir(&journal).exists());
}

#[test]
fn leaves_what_is_in_both_places() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let path = "archive/2026/11/2026-11-01.journal.zst";
  put(&journal.join(path), "old");
  put(&Dgg::dir(&journal).join(path), "new");
  drop(open(&dir.path().join(LEGACY_FILE)).unwrap());
  drop(open(&dir.path().join(FILE)).unwrap());
  let report = run(dir.path(), false);
  assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
  assert_eq!(fs::read_to_string(journal.join(path)).unwrap(), "old");
  assert!(dir.path().join(LEGACY_FILE).exists());
}

#[test]
fn a_dry_run_moves_nothing() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let header = Header::new(Variant::Dgg, v0::VERSION).text();
  put(&journal.join("live/2026-11-24/a.journal"), &header);
  drop(open(&dir.path().join(LEGACY_FILE)).unwrap());
  let report = run(dir.path(), true);
  assert_eq!(report.moved, 1);
  assert!(journal.join("live/2026-11-24/a.journal").exists());
  assert!(!Dgg::dir(&journal).exists());
  assert!(dir.path().join(LEGACY_FILE).exists());
}
