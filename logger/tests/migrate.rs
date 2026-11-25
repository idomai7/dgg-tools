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
    Header, Variant, Version, Writer,
    record::{
      kick,
      v0::{self, Body},
    },
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

/// dgg's journals, as they were before they were by channel: `files`
/// under `dgg/`, each with what it holds, and the index.
fn before_channels(data: &Path, files: &[(&str, &str)]) {
  let dir = Dgg::dir(&data.join("journal"));
  for (path, text) in files {
    put(&dir.join(path), text);
  }
  drop(open(&data.join("index-dgg.sqlite")).unwrap());
}

#[test]
fn moves_dgg_into_destiny_ggs_directory() {
  let dir = TempDir::new("logger-migrate");
  let dgg = Header::new(Variant::Dgg, v0::VERSION).text();
  let synced = "synced-archives dgg.0.2\u{1e}\n";
  let files = [
    ("live/2026-11-24/a.journal", dgg.as_str()),
    ("archive/2026/11/2026-11-20.journal.zst", "zst"),
    ("archive/2026/11/manifest", "manifest"),
    ("control/synced-archives", synced),
    ("control/gone.live-lock", ""),
    ("control/sync-2026-11-20.download", "part"),
    ("backfill/live/2026-11-20/b.journal", dgg.as_str()),
    ("backfill/control/b.lock", ""),
  ];
  before_channels(dir.path(), &files);
  let journal = dir.path().join("journal");
  // A stale status, from a capture long gone.
  let old_control = control::dir(&Dgg::dir(&journal));
  let stale = control::Status {
    instance: "gone".into(),
    started: 1,
    heartbeat: 2,
    healthy_since: None,
    build: None,
    listening: None,
  };
  control::write(&old_control, &stale).unwrap();

  // A dry run changes nothing.
  let report = run(dir.path(), true);
  assert_eq!(report.moved, 5);
  assert!(Dgg::dir(&journal).join("live").exists());

  let report = run(dir.path(), false);
  assert_eq!(report.failed, Vec::<String>::new());
  assert_eq!((report.moved, report.left.len()), (5, 0));
  let channel = Dgg::channel_dir(&Dgg::dir(&journal), DESTINY);
  for (path, text) in [
    ("live/2026-11-24/a.journal", dgg.as_str()),
    ("archive/2026/11/2026-11-20.journal.zst", "zst"),
    ("archive/2026/11/manifest", "manifest"),
    ("control/synced-archives", synced),
    ("backfill/live/2026-11-20/b.journal", dgg.as_str()),
  ] {
    assert_eq!(
      fs::read_to_string(channel.join(path)).unwrap(),
      text,
      "{path}"
    );
  }
  // Nothing's left of the old layout, and the index moved as it was.
  for old in ["live", "archive", "control", "backfill"] {
    assert!(!Dgg::dir(&journal).join(old).exists(), "{old}");
  }
  assert!(!dir.path().join("index-dgg.sqlite").exists());
  assert_eq!(report.indexes, [(file(DESTINY), SCHEMA_VERSION)]);
  // And again, nothing to do.
  assert_eq!(run(dir.path(), false).moved, 0);
}

#[test]
fn leaves_what_a_capture_still_writes() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  let mut writer = Writer::capture(Dgg::dir(&journal), "old").unwrap();
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  writer.sync().unwrap();
  let report = run(dir.path(), false);
  assert_eq!(report.failed, Vec::<String>::new());
  assert_eq!(report.moved, 0);
  assert_eq!(report.left.len(), 1);
  // Its capture gone, a later migrate moves it.
  drop(writer);
  let report = run(dir.path(), false);
  assert_eq!((report.moved, report.left.len()), (1, 0));
  let channel = Dgg::channel_dir(&Dgg::dir(&journal), DESTINY);
  assert!(channel.join("live/1970-01-01/old.journal").exists());
  assert!(!Dgg::dir(&journal).join("live").exists());
}

#[test]
fn refuses_to_move_over_a_file() {
  let dir = TempDir::new("logger-migrate");
  let path = "archive/2026/11/2026-11-20.journal.zst";
  before_channels(dir.path(), &[(path, "old")]);
  let journal = dir.path().join("journal");
  let channel = Dgg::channel_dir(&Dgg::dir(&journal), DESTINY);
  put(&channel.join(path), "new");
  drop(open(&dir.path().join(file(DESTINY))).unwrap());
  let report = run(dir.path(), false);
  assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
  assert_eq!(
    fs::read_to_string(Dgg::dir(&journal).join(path)).unwrap(),
    "old"
  );
  assert_eq!(fs::read_to_string(channel.join(path)).unwrap(), "new");
  assert!(dir.path().join("index-dgg.sqlite").exists());
}

#[test]
fn says_what_is_left_of_the_old_layout() {
  let dir = TempDir::new("logger-migrate");
  let journal = dir.path().join("journal");
  put(&journal.join("live/2026-11-24/a.journal"), "");
  put(&dir.path().join("index.sqlite"), "");
  let report = run(dir.path(), false);
  assert_eq!(report.failed, Vec::<String>::new());
  assert_eq!(
    report.old,
    [journal.join("live"), dir.path().join("index.sqlite")]
  );
  // Left as it was.
  assert!(journal.join("live/2026-11-24/a.journal").exists());
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
