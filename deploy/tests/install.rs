//! The pieces of an install: builds, waiting for health, pruning, and the
//! scheduled task.

use {
  ::deploy::{
    install::{BINARIES, build_name, copy_build, prune, wait_healthy},
    log::now_ms,
  },
  ::std::{
    env::consts::EXE_SUFFIX,
    fs,
    path::{Path, PathBuf},
    time::Duration,
  },
};

struct TempDir(PathBuf);

impl TempDir {
  fn new(name: &str) -> Self {
    let path = ::std::env::temp_dir()
      .join(format!("deploy-install-{name}-{}", ::std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    Self(path)
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn exe(dir: &Path, name: &str) -> PathBuf {
  dir.join(format!("{name}{EXE_SUFFIX}"))
}

#[test]
fn names_builds_by_time_and_version() {
  // 2026-11-08 20:04:00.123Z
  assert_eq!(
    build_name(1_794_168_240_123),
    format!("20261108-200400-{}", env!("CARGO_PKG_VERSION"))
  );
}

#[test]
fn copies_a_whole_build_or_nothing() {
  let dir = TempDir::new("copy");
  let from = dir.0.join("release");
  fs::create_dir_all(&from).unwrap();
  for binary in &BINARIES[1..] {
    fs::write(exe(&from, binary), binary).unwrap();
  }
  let bin = dir.0.join("bin");
  let error = copy_build(&from, &bin, "b1").unwrap_err().to_string();
  assert!(error.starts_with("dgg-supervisor isn't in"), "{error}");
  assert!(!bin.join("b1").exists());

  fs::write(exe(&from, "dgg-supervisor"), "s").unwrap();
  let build = copy_build(&from, &bin, "b1").unwrap();
  assert_eq!(build, bin.join("b1"));
  for binary in BINARIES {
    assert!(exe(&build, binary).exists(), "{binary}");
  }
}

#[test]
fn prunes_old_builds() {
  let dir = TempDir::new("prune");
  for build in [
    "20260101-000000-0.1.0",
    "20260102-000000-0.1.0",
    "20260103-000000-0.1.0",
  ] {
    fs::create_dir_all(dir.0.join(build)).unwrap();
    fs::write(dir.0.join(build).join("x"), "").unwrap();
  }
  assert_eq!(prune(&dir.0, 2).unwrap(), Vec::<PathBuf>::new());
  let mut left: Vec<_> = fs::read_dir(&dir.0)
    .unwrap()
    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
    .collect();
  left.sort();
  assert_eq!(left, ["20260102-000000-0.1.0", "20260103-000000-0.1.0"]);
  assert!(prune(&dir.0.join("missing"), 1).unwrap().is_empty());
}

#[test]
fn waits_for_a_new_healthy_capture() {
  let dir = TempDir::new("healthy");
  let control = dir.0.join("control");
  fs::create_dir_all(&control).unwrap();
  let now = now_ms();
  let write = |instance: &str, started: i64, healthy: bool| {
    let mut text = format!(
      "capture-status 0.0\ninstance {instance}\nstarted {started}\nheartbeat {now}\n"
    );
    if healthy {
      text += &format!("healthy-since {started}\n");
    }
    fs::write(control.join(format!("{instance}.status")), text).unwrap();
  };
  // An old one, healthy, and a new one still connecting: not yet.
  write("old-1", now - 600_000, true);
  write("new-2", now, false);
  let short = Duration::from_millis(600);
  assert_eq!(wait_healthy(&dir.0, now - 1000, short).unwrap(), None);
  write("new-2", now, true);
  assert_eq!(
    wait_healthy(&dir.0, now - 1000, short).unwrap().as_deref(),
    Some("new-2")
  );
}

#[cfg(windows)]
mod task {
  use {
    ::deploy::install::task::{
      Task, arguments, delete, find, parse, register, xml,
    },
    ::std::path::PathBuf,
  };

  fn task() -> Task {
    let root = PathBuf::from(r"C:\Users\some one\AppData\Local\dgg-tools");
    Task {
      supervisor: root.join(r"bin\20261109-120000-0.1.0\dgg-supervisor.exe"),
      data: root.join("data"),
      logs: root.join("logs"),
    }
  }

  #[test]
  fn reads_back_what_it_writes() {
    let task = task();
    assert_eq!(
      arguments(&task),
      r#"--data "C:\Users\some one\AppData\Local\dgg-tools\data" --logs "C:\Users\some one\AppData\Local\dgg-tools\logs""#
    );
    let xml = xml(&task, r"HOST\some one");
    assert!(xml.contains(r"<UserId>HOST\some one</UserId>"));
    assert!(xml.contains("&quot;"));
    assert_eq!(parse(&xml), Some(task));
    assert_eq!(parse(&xml.replace("dgg-supervisor", "something")), None);
  }

  /// Against Task Scheduler itself: registered under a name of its own (and
  /// never run), found, deleted.
  #[test]
  fn registers_finds_and_deletes() {
    let name = format!("dgg-logger-test-{}", ::std::process::id());
    let task = task();
    register(&name, &task).unwrap();
    let found = find(&name);
    delete(&name).unwrap();
    assert_eq!(found.unwrap(), Some(task));
    assert_eq!(find(&name).unwrap(), None);
    // Deleting what isn't there is fine.
    delete(&name).unwrap();
  }
}
