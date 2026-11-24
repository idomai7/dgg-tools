//! Migrating, while there's nothing to migrate: it checks the version of
//! each kind of file, and the index's schema, under a supervisor's locks.

use {
  ::common::{temp_dir::TempDir, time::now_ms},
  ::logger::{
    BUILD, control,
    index::{SCHEMA_VERSION, open, user_version},
    migrate::{Report, migrate},
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

/// Writes `text` at `path`, making its directory.
fn put(path: &Path, text: &str) {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, text).unwrap();
}

#[test]
fn says_each_kind_of_file() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let dgg = Header::new(Variant::Dgg, v0::VERSION).text();
  let old = Header::new(Variant::Dgg, Version { major: 0, minor: 2 }).text();
  put(&journal.join("live/2026-11-24/a.journal"), &dgg);
  put(&journal.join("live/2026-11-24/b.journal"), &old);
  put(&journal.join("backfill/live/2026-11-20/c.journal"), &dgg);
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
  control::write(&control::dir(&journal), &status("old", "0.0.1-abc")).unwrap();
  control::write(&control::dir(&journal), &status("new", BUILD)).unwrap();

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
    "journal: 2 live files, journal dgg.0.3; 1 at another version",
    "journal: live/2026-11-24/b.journal is journal dgg.0.2",
    "journal: 2 capture statuses, capture-status dgg.0.2",
    "backfill: 1 live file, journal dgg.0.3",
    "kick someone: 1 live file, journal kick.0.1",
    "index: none yet",
  ] {
    assert!(
      said.contains(line),
      "{line:?} not in:
{said}"
    );
  }
  assert_eq!(
    report.unreadable,
    [journal.join("live/2026-11-24/b.journal")]
  );
  assert_eq!(report.failed, Vec::<String>::new());
}
