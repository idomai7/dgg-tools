//! Migrating, while there's nothing to migrate: it checks the index's
//! schema, under a supervisor's locks.

use {
  ::common::temp_dir::TempDir,
  ::logger::{
    index::{SCHEMA_VERSION, open, user_version},
    migrate::{Report, migrate},
  },
  ::logger_journal::Version,
  ::std::{fs::File, path::Path, sync::mpsc, thread, time::Duration},
};

fn run(data: &Path, dry_run: bool) -> Report {
  migrate(data, dry_run, &mut |line| eprintln!("{line}")).unwrap()
}

#[test]
fn nothing_to_migrate() {
  let dir = TempDir::new("logger-migrate");
  assert_eq!(run(dir.path(), false), Report::default());
  drop(open(&dir.path().join("index.sqlite")).unwrap());
  for dry_run in [false, true] {
    let report = run(dir.path(), dry_run);
    assert_eq!(report.failed, Vec::<String>::new());
    assert_eq!(report.index, Some(SCHEMA_VERSION));
  }
}

#[test]
fn an_index_at_another_schema_fails() {
  let dir = TempDir::new("logger-migrate");
  let db = open(&dir.path().join("index.sqlite")).unwrap();
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
