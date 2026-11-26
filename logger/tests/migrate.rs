//! Migrating: it checks the version of each kind of file and each index's
//! schema, and what's left of an older layout, under a supervisor's locks.

use {
  ::common::{temp_dir::TempDir, time::now_ms},
  ::logger::{
    BUILD, control,
    dgg::{
      Dgg,
      channels::DESTINY,
      index::{SCHEMA_VERSION, file, open},
    },
    index::user_version,
    migrate::{Report, migrate},
    service::Service,
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
  // An index is checked as its channel's, which has a directory.
  fs::create_dir_all(Dgg::channel_dir(
    &Dgg::dir(&dir.path().join("journal")),
    DESTINY,
  ))
  .unwrap();
  drop(open(&dir.path().join(file(DESTINY))).unwrap());
  for dry_run in [false, true] {
    let report = run(dir.path(), dry_run);
    assert_eq!(report.failed, Vec::<String>::new());
    assert_eq!(report.indexes, [(file(DESTINY), SCHEMA_VERSION)]);
  }
}

#[test]
fn an_index_at_another_schema_fails() {
  let dir = TempDir::new("logger-migrate");
  fs::create_dir_all(Dgg::channel_dir(
    &Dgg::dir(&dir.path().join("journal")),
    DESTINY,
  ))
  .unwrap();
  let db = open(&dir.path().join(file(DESTINY))).unwrap();
  let old = Version { major: 0, minor: 5 };
  db.pragma_update(None, "user_version", user_version(old))
    .unwrap();
  drop(db);
  let report = run(dir.path(), false);
  assert_eq!(report.indexes, [(file(DESTINY), old)]);
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
  let dgg_dir = Dgg::channel_dir(&Dgg::dir(&journal), DESTINY);
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
    listening: None,
  };
  let control = control::dir(&dgg_dir);
  control::write(&control, &status("old", "0.0.1-abc")).unwrap();
  control::write(&control, &status("new", BUILD)).unwrap();

  let mut said = Vec::new();
  let report =
    migrate(dir.path(), false, &mut |_, line| said.push(line.to_owned()))
      .unwrap();
  let said = said.join("\n");
  assert!(said.starts_with(&format!("build {BUILD}:")), "{said}");
  for line in [
    "dgg destiny.gg: 2 live files, journal dgg.0.3; 1 at another version",
    "dgg destiny.gg: live/2026-11-24/b.journal is journal dgg.0.2",
    "dgg destiny.gg: 2 capture statuses, capture-status dgg.0.2",
    "dgg destiny.gg backfill: 1 live file, journal dgg.0.3",
    "kick someone: 1 live file, journal kick.0.1",
    "index-dgg-destiny.gg.sqlite: none yet",
    "index-kick-someone.sqlite: none yet",
  ] {
    assert!(said.contains(line), "{line:?} not in:\n{said}");
  }
  assert_eq!(
    report.unreadable,
    [dgg_dir.join("live/2026-11-24/b.journal")]
  );
  assert_eq!(report.failed, Vec::<String>::new());
}

#[test]
fn says_what_is_left_of_the_old_layout() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  put(&journal.join("live/2026-11-24/a.journal"), "");
  put(&dir.path().join("index.sqlite"), "");
  // And from before dgg's journals were by channel.
  put(&journal.join("dgg/live/2026-11-25/b.journal"), "");
  put(&dir.path().join("index-dgg.sqlite"), "");
  let report = run(dir.path(), false);
  assert_eq!(report.failed, Vec::<String>::new());
  assert_eq!(
    report.old,
    [
      journal.join("live"),
      dir.path().join("index.sqlite"),
      journal.join("dgg/live"),
      dir.path().join("index-dgg.sqlite"),
    ]
  );
  // Left as they were.
  assert!(journal.join("live/2026-11-24/a.journal").exists());
  assert!(journal.join("dgg/live/2026-11-25/b.journal").exists());
  assert_eq!(report.moved, 0);
}

#[test]
fn deletes_kicks_index_of_every_channel() {
  let dir = TempDir::new("logger-migrate");
  let old = dir.path().join(::logger::kick::index::OLD_FILE);
  drop(::logger::kick::index::open(&old).unwrap());
  put(&dir.path().join("index-kick.sqlite-wal"), "");

  // A dry run says so, and changes nothing.
  let report = run(dir.path(), true);
  assert!(report.kick_index_removed);
  assert!(old.exists());

  let report = run(dir.path(), false);
  assert_eq!(report.failed, Vec::<String>::new());
  assert!(report.kick_index_removed);
  assert!(!old.exists());
  assert!(!dir.path().join("index-kick.sqlite-wal").exists());
  assert!(!run(dir.path(), false).kick_index_removed);
}
