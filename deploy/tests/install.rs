//! The pieces of an install: builds, waiting for health, pruning, and the
//! scheduled task.

use {
  ::common::temp_dir::TempDir,
  ::common::time::now_ms,
  ::deploy::install::{
    BINARIES, build_name, copy_build, prune, wait_healthy, wait_kick_healthy,
  },
  ::std::{
    env::consts::EXE_SUFFIX,
    fs,
    path::{Path, PathBuf},
    time::Duration,
  },
};

fn exe(dir: &Path, name: &str) -> PathBuf {
  dir.join(format!("{name}{EXE_SUFFIX}"))
}

#[test]
fn names_builds_by_time_and_commit() {
  // 2026-11-08 20:04:00.123Z
  assert_eq!(
    build_name(1_794_168_240_123),
    format!("20261108-200400-{}", ::deploy::BUILD)
  );
}

#[test]
fn copies_a_whole_build_or_nothing() {
  let dir = TempDir::new("deploy-install-copy");
  let from = dir.path().join("release");
  fs::create_dir_all(&from).unwrap();
  for binary in &BINARIES[1..] {
    fs::write(exe(&from, binary), binary).unwrap();
  }
  let bin = dir.path().join("bin");
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
  let dir = TempDir::new("deploy-install-prune");
  for build in [
    "20260101-000000-0.1.0",
    "20260102-000000-0.1.0",
    "20260103-000000-0.1.0",
  ] {
    fs::create_dir_all(dir.path().join(build)).unwrap();
    fs::write(dir.path().join(build).join("x"), "").unwrap();
  }
  assert_eq!(prune(dir.path(), 2).unwrap(), Vec::<PathBuf>::new());
  let mut left: Vec<_> = fs::read_dir(dir.path())
    .unwrap()
    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
    .collect();
  left.sort();
  assert_eq!(left, ["20260102-000000-0.1.0", "20260103-000000-0.1.0"]);
  assert!(prune(&dir.path().join("missing"), 1).unwrap().is_empty());
}

#[test]
fn waits_for_a_new_healthy_capture() {
  let dir = TempDir::new("deploy-install-healthy");
  let control = dir.path().join("dgg").join("control");
  fs::create_dir_all(&control).unwrap();
  let now = now_ms();
  let write = |instance: &str, started: i64, healthy: bool| {
    let status = ::logger::control::Status {
      instance: instance.into(),
      started,
      heartbeat: now,
      healthy_since: healthy.then_some(started),
      build: None,
    };
    fs::write(control.join(format!("{instance}.status")), status.encode())
      .unwrap();
  };
  // An old one, healthy, and a new one still connecting: not yet.
  write("old-1", now - 600_000, true);
  write("new-2", now, false);
  let short = Duration::from_millis(600);
  assert_eq!(wait_healthy(dir.path(), now - 1000, short).unwrap(), None);
  write("new-2", now, true);
  assert_eq!(
    wait_healthy(dir.path(), now - 1000, short)
      .unwrap()
      .as_deref(),
    Some("new-2")
  );
}

#[test]
fn waits_for_a_new_healthy_kick_capture() {
  use ::logger_journal::Variant;

  let dir = TempDir::new("deploy-install-kick-healthy");
  let control = dir.path().join("one").join("control");
  fs::create_dir_all(&control).unwrap();
  let now = now_ms();
  let write = |instance: &str, variant: Variant| {
    let status = ::logger::control::Status {
      instance: instance.into(),
      started: now,
      heartbeat: now,
      healthy_since: Some(now),
      build: None,
    };
    let text = status.encode_as(variant);
    fs::write(control.join(format!("{instance}.status")), text).unwrap();
  };
  let short = Duration::from_millis(600);
  // dgg's status in a Kick channel's control isn't Kick's capture.
  write("dgg-2", Variant::Dgg);
  let wait = || wait_kick_healthy(dir.path(), "one", now - 1000, short);
  assert_eq!(wait().unwrap(), None);
  write("kick-2", Variant::Kick);
  assert_eq!(wait().unwrap().as_deref(), Some("kick-2"));
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
      peers: Some(root),
    }
  }

  #[test]
  fn reads_back_what_it_writes() {
    let task = task();
    assert_eq!(
      arguments(&task),
      r#"--data "C:\Users\some one\AppData\Local\dgg-tools\data" --logs "C:\Users\some one\AppData\Local\dgg-tools\logs" --peers "C:\Users\some one\AppData\Local\dgg-tools""#
    );
    let xml = xml(&task, r"HOST\some one");
    assert!(xml.contains(r"<UserId>HOST\some one</UserId>"));
    assert!(xml.contains("&quot;"));
    assert_eq!(parse(&xml), Some(task));
    assert_eq!(parse(&xml.replace("dgg-supervisor", "something")), None);
  }

  /// A task from before syncing: no peers.
  #[test]
  fn reads_tasks_without_peers() {
    let task = Task {
      peers: None,
      ..task()
    };
    assert!(!arguments(&task).contains("--peers"));
    assert_eq!(parse(&xml(&task, "someone")), Some(task));
  }

  /// A task from before several peers: its peer file's directory.
  #[test]
  fn reads_tasks_with_a_peer_file() {
    let task = task();
    let old = arguments(&task).replace(
      r#"--peers "C:\Users\some one\AppData\Local\dgg-tools""#,
      r#"--peer-file "C:\Users\some one\AppData\Local\dgg-tools\peer.env""#,
    );
    assert!(old.contains("--peer-file"), "{old}");
    let parsed =
      ::deploy::install::task::parse_arguments(task.supervisor.clone(), &old);
    assert_eq!(parsed, Some(task));
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

#[cfg(windows)]
mod service {
  use {
    ::deploy::install::{
      service::{account, command_line, parse},
      task::Task,
    },
    ::std::path::PathBuf,
  };

  #[test]
  fn reads_back_its_command_line() {
    let root = PathBuf::from(r"C:\ProgramData\dgg-tools");
    let task = Task {
      supervisor: root.join(r"bin\20261109-120000-0.1.0\dgg-supervisor.exe"),
      data: PathBuf::from(r"D:\chat data"),
      logs: root.join("logs"),
      peers: None,
    };
    let line = command_line(&task);
    assert_eq!(
      line,
      r#""C:\ProgramData\dgg-tools\bin\20261109-120000-0.1.0\dgg-supervisor.exe" --service --data "D:\chat data" --logs "C:\ProgramData\dgg-tools\logs""#
    );
    assert_eq!(parse(&line), Some(task));
    assert_eq!(parse(&line.replace("--service ", "")), None);
    assert_eq!(
      parse(r#""C:\x\other.exe" --service --data "a" --logs "b""#),
      None
    );
    assert_eq!(account("dgg-logger"), r"NT SERVICE\dgg-logger");
  }
}

#[cfg(windows)]
mod acl {
  use {
    super::TempDir,
    ::deploy::install::acl::{elevated, links, owner, secret},
    ::std::{fs, process::Command},
  };

  #[test]
  fn finds_links_without_following_them() {
    let dir = TempDir::new("deploy-install-links");
    fs::create_dir_all(dir.path().join(r"a\b")).unwrap();
    fs::write(dir.path().join(r"a\file"), "").unwrap();
    assert!(links(dir.path()).unwrap().is_empty());
    // A junction (which needs no elevation) to somewhere big.
    let junction = dir.path().join(r"a\b\elsewhere");
    let made = Command::new("cmd.exe")
      .args(["/c", "mklink", "/J"])
      .arg(&junction)
      .arg(r"C:\Windows")
      .output()
      .unwrap();
    assert!(made.status.success());
    assert_eq!(
      links(dir.path()).unwrap(),
      ::std::slice::from_ref(&junction)
    );
    fs::remove_dir(&junction).unwrap();
  }

  /// Needs an elevated prompt (taking ownership for Administrators), so
  /// skipped without one.
  #[test]
  fn keeps_secrets_from_users() {
    if !elevated() {
      return;
    }
    let dir = TempDir::new("deploy-install-secret");
    let file = dir.path().join("peer.env");
    fs::write(&file, "").unwrap();
    secret(&file, &[r"NT AUTHORITY\LOCAL SERVICE"]).unwrap();
    let shown = Command::new("icacls.exe").arg(&file).output().unwrap();
    let shown = String::from_utf8_lossy(&shown.stdout);
    assert!(shown.contains(r"NT AUTHORITY\LOCAL SERVICE:(R)"), "{shown}");
    assert!(!shown.contains(r"BUILTIN\Users"), "{shown}");
    assert!(!shown.contains("(I)"), "inherits: {shown}");
  }

  #[test]
  fn reads_owners() {
    let dir = TempDir::new("deploy-install-owner");
    // Made by this (unelevated) user, so owned by them, not Administrators.
    let sid = owner(dir.path()).unwrap();
    assert!(sid.starts_with("S-1-5-"), "{sid}");
    if !elevated() {
      assert_ne!(sid, "S-1-5-32-544");
    }
  }
}

mod planning {
  use {
    ::deploy::{
      install::{Installed, Options, Scope, plan},
      paths::Paths,
    },
    ::std::path::PathBuf,
  };

  fn options(scope: Scope) -> Options {
    Options {
      scope,
      ..Options::default()
    }
  }

  #[test]
  fn keeps_an_install_of_the_same_scope_where_it_is() {
    let custom = Paths {
      root: PathBuf::from(r"D:\logger"),
      data: PathBuf::from(r"E:\chat"),
      logs: PathBuf::from(r"D:\logger\logs"),
    };
    let current = Installed::Task(custom.clone());
    assert_eq!(plan(&options(Scope::User), Some(&current)), custom);
    // Another scope starts from its own defaults.
    let machine = plan(&options(Scope::Machine), Some(&current));
    assert_eq!(machine, Paths::under(Scope::Machine.default_root()));
    assert_ne!(machine.data, custom.data);
  }

  #[test]
  fn defaults_and_overrides() {
    assert_eq!(
      plan(&options(Scope::User), None),
      Paths::under(Scope::User.default_root())
    );
    let mut told = options(Scope::User);
    told.root = Some(PathBuf::from(r"D:\x"));
    told.logs = Some(PathBuf::from(r"D:\logs"));
    assert_eq!(
      plan(&told, None),
      Paths {
        root: PathBuf::from(r"D:\x"),
        data: PathBuf::from(r"D:\x\data"),
        logs: PathBuf::from(r"D:\logs"),
      }
    );
  }
}

mod moving {
  use {
    super::TempDir,
    ::common::fs::same,
    ::deploy::install::moving::{collisions, merge, merge_logs},
    ::std::{fs, path::PathBuf},
  };

  fn write(dir: &TempDir, path: &str, text: &str) {
    let path = dir.path().join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
  }

  #[test]
  fn merges_data_unless_a_file_is_in_both() {
    let dir = TempDir::new("deploy-install-merge");
    write(&dir, "old/journal/live/2026-11-09/a-1.journal", "a");
    write(&dir, "old/index.sqlite", "i");
    write(&dir, "new/journal/live/2026-11-09/b-2.journal", "b");
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    assert!(collisions(&old, &new).unwrap().is_empty());
    merge(&old, &new).unwrap();
    assert!(!old.exists());
    assert_eq!(
      fs::read_to_string(new.join("journal/live/2026-11-09/a-1.journal"))
        .unwrap(),
      "a"
    );
    assert!(new.join("journal/live/2026-11-09/b-2.journal").exists());
    assert!(new.join("index.sqlite").exists());

    write(&dir, "old/index.sqlite", "again");
    write(&dir, "old/other", "o");
    assert_eq!(
      collisions(&old, &new).unwrap(),
      [PathBuf::from("index.sqlite")]
    );
    assert!(merge(&old, &new).is_err());
    // Nothing moved.
    assert!(old.join("other").exists());
    assert_eq!(fs::read_to_string(new.join("index.sqlite")).unwrap(), "i");
    assert!(same(&new, &new) && !same(&old, &new));
  }

  #[test]
  fn merges_logs_oldest_first() {
    let dir = TempDir::new("deploy-install-logs");
    write(&dir, "old/capture.log", "old\n");
    write(&dir, "old/index.log", "index\n");
    write(&dir, "new/capture.log", "new\n");
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    merge_logs(&old, &new).unwrap();
    assert_eq!(
      fs::read_to_string(new.join("capture.log")).unwrap(),
      "old\nnew\n"
    );
    assert_eq!(
      fs::read_to_string(new.join("index.log")).unwrap(),
      "index\n"
    );
    assert!(!old.exists());
  }
}

#[cfg(windows)]
#[test]
fn bridges_run_detached_into_their_log() {
  use ::deploy::install::bridge;

  let dir = TempDir::new("deploy-install-bridge");
  // Something to stand in for dgg-logger: it complains about `capture`,
  // into the log, and exits.
  fs::copy(
    r"C:\Windows\System32\whoami.exe",
    exe(dir.path(), "dgg-logger"),
  )
  .unwrap();
  let log = dir.path().join("bridge.log");
  let mut child =
    bridge::start(dir.path(), &dir.path().join("data"), &log).unwrap();
  assert!(bridge::wait_gone(&mut child, Duration::from_secs(10)));
  assert!(!fs::read(&log).unwrap().is_empty());

  let log = dir.path().join("kick-bridge.log");
  let (data, list) = (dir.path().join("data"), dir.path().join("list"));
  let mut child = bridge::start_kick(dir.path(), &data, &list, &log).unwrap();
  assert!(bridge::wait_gone(&mut child, Duration::from_secs(10)));
  assert!(!fs::read(&log).unwrap().is_empty());
}
