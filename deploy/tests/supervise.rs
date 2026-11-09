//! The supervisor, with small scripts standing in for the logger's programs.

use {
  ::deploy::supervise::{Backoff, Config, Exit, Program, Stop, run},
  ::std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
  },
};

struct TempDir(PathBuf);

impl TempDir {
  fn new(name: &str) -> Self {
    let path = ::std::env::temp_dir()
      .join(format!("deploy-supervise-{name}-{}", ::std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    Self(path)
  }

  fn log(&self, name: &str) -> String {
    fs::read_to_string(self.0.join("logs").join(format!("{name}.log")))
      .unwrap_or_default()
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

/// What a script does, in either shell.
enum Step<'a> {
  Say(&'a str),
  Sleep(u32),
  /// Exits 0 if `marker` exists, else creates it and exits 1.
  FailOnce(&'a str),
  /// Says its arguments, then the variable `name`.
  SayArgsAnd(&'a str),
  Exit(u32),
}

/// A script in `dir` doing `steps`.
fn script(dir: &Path, name: &str, steps: &[Step]) -> Program {
  let mut text = String::new();
  #[cfg(windows)]
  {
    text += "@echo off\r\n";
    for step in steps {
      text += &match step {
        Step::Say(words) => format!("echo {words}\r\n"),
        // ping waits about a second between its n pings.
        Step::Sleep(seconds) => {
          format!("ping -n {} 127.0.0.1 >nul\r\n", seconds + 1)
        }
        Step::FailOnce(marker) => format!(
          "if exist \"{0}\" exit /b 0\r\ntype nul > \"{0}\"\r\nexit /b 1\r\n",
          dir.join(marker).display()
        ),
        Step::SayArgsAnd(name) => format!("echo %* %{name}%\r\n"),
        Step::Exit(code) => format!("exit /b {code}\r\n"),
      };
    }
  }
  #[cfg(not(windows))]
  {
    text += "#!/bin/sh\n";
    for step in steps {
      text += &match step {
        Step::Say(words) => format!("echo {words}\n"),
        Step::Sleep(seconds) => format!("sleep {seconds}\n"),
        Step::FailOnce(marker) => format!(
          "[ -e '{0}' ] && exit 0\ntouch '{0}'\nexit 1\n",
          dir.join(marker).display()
        ),
        Step::SayArgsAnd(name) => format!("echo \"$@\" \"${name}\"\n"),
        Step::Exit(code) => format!("exit {code}\n"),
      };
    }
  }
  let extension = if cfg!(windows) { "cmd" } else { "sh" };
  let path = dir.join(format!("{name}.{extension}"));
  fs::write(&path, text).unwrap();
  #[cfg(unix)]
  {
    use ::std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
  }
  Program {
    path,
    ..Program::default()
  }
}

fn config(dir: &TempDir, capture: Program, index: Program) -> Config {
  let quiet = script(&dir.0, "compact", &[Step::Exit(0)]);
  let mut config =
    Config::new(&dir.0, &dir.0.join("data"), &dir.0.join("logs"));
  config.capture = capture;
  config.index = index;
  config.compact = quiet;
  config.compact_first = Duration::from_secs(3600);
  config.backoff = Backoff {
    first: Duration::from_millis(50),
    max: Duration::from_millis(200),
    reset_after: Duration::from_secs(60),
  };
  config
}

#[test]
fn exits_when_capture_hands_off() {
  let dir = TempDir::new("handoff");
  let capture = script(
    &dir.0,
    "capture",
    &[Step::Say("capturing"), Step::Sleep(1), Step::Exit(0)],
  );
  let index =
    script(&dir.0, "index", &[Step::Say("indexing"), Step::Sleep(20)]);
  let started = Instant::now();
  let exit = run(&config(&dir, capture, index), &Stop::new()).unwrap();
  assert_eq!(exit, Exit::HandedOff);
  // The index is killed, not waited for.
  assert!(started.elapsed() < Duration::from_secs(10));
  assert!(dir.log("capture").contains(" capturing"));
  assert!(dir.log("index").contains(" indexing"));
  let supervisor = dir.log("supervisor");
  assert!(supervisor.contains("capture handed off; stopping"));
  assert!(supervisor.contains("index: stopped"));
}

#[test]
fn restarts_programs_that_fail() {
  let dir = TempDir::new("restart");
  let capture = script(&dir.0, "capture", &[Step::FailOnce("failed")]);
  let index = script(&dir.0, "index", &[Step::Sleep(20)]);
  let exit = run(&config(&dir, capture, index), &Stop::new()).unwrap();
  assert_eq!(exit, Exit::HandedOff);
  let supervisor = dir.log("supervisor");
  assert!(supervisor.contains("capture: exited, exit code: 1"));
  assert!(supervisor.contains("capture: restarting in 0.05s"));
}

#[test]
fn stops_on_request() {
  let dir = TempDir::new("stop");
  let capture = script(&dir.0, "capture", &[Step::Sleep(20)]);
  let index = script(&dir.0, "index", &[Step::Sleep(20)]);
  let stop = Stop::new();
  let requester = stop.clone();
  thread::spawn(move || {
    thread::sleep(Duration::from_millis(500));
    requester.request();
  });
  let started = Instant::now();
  let exit = run(&config(&dir, capture, index), &stop).unwrap();
  assert_eq!(exit, Exit::Stopped);
  assert!(started.elapsed() < Duration::from_secs(10));
  let supervisor = dir.log("supervisor");
  assert!(supervisor.contains("capture: stopped"));
  assert!(supervisor.contains("index: stopped"));
}

#[test]
fn one_index_at_a_time() {
  let dir = TempDir::new("lock");
  let first = config(
    &dir,
    script(&dir.0, "capture", &[Step::Sleep(20)]),
    script(
      &dir.0,
      "index",
      &[Step::Say("first index"), Step::Sleep(20)],
    ),
  );
  let mut second = first.clone();
  second.logs = dir.0.join("logs2");
  second.index = script(&dir.0, "index2", &[Step::Say("second index")]);
  let (stop_first, stop_second) = (Stop::new(), Stop::new());
  thread::scope(|scope| {
    scope.spawn(|| run(&first, &stop_first).unwrap());
    thread::sleep(Duration::from_millis(1000));
    scope.spawn(|| run(&second, &stop_second).unwrap());
    thread::sleep(Duration::from_millis(1500));
    let waiting =
      fs::read_to_string(dir.0.join("logs2/supervisor.log")).unwrap();
    assert!(waiting.contains("another supervisor has it"), "{waiting}");
    assert!(
      !dir.0.join("logs2/index.log").exists() || {
        !fs::read_to_string(dir.0.join("logs2/index.log"))
          .unwrap()
          .contains("second index")
      }
    );
    // Once the first stops, the second takes over the index.
    stop_first.request();
    thread::sleep(Duration::from_millis(2500));
    let index = fs::read_to_string(dir.0.join("logs2/index.log")).unwrap();
    assert!(index.contains("second index"), "{index}");
    stop_second.request();
  });
}

#[test]
fn runs_the_loggers_commands() {
  let bin = Path::new("bin");
  let config = Config::new(bin, Path::new("data"), Path::new("logs"));
  let logger =
    bin.join(format!("dgg-logger{}", ::std::env::consts::EXE_SUFFIX));
  let args = |program: &::deploy::supervise::Program| {
    assert_eq!(program.path, logger);
    program
      .args
      .iter()
      .map(|arg| arg.to_string_lossy().into_owned())
      .collect::<Vec<_>>()
      .join(" ")
  };
  assert_eq!(args(&config.capture), "capture --data data");
  assert_eq!(args(&config.index), "index --follow --data data");
  assert_eq!(args(&config.compact), "compact --data data");
  assert_eq!(args(&config.sync), "sync --data data");
  assert_eq!(config.peer_file, None);
}

/// Runs `run` with a capture that hands off after a few seconds, and a sync
/// that says how it was called, due soon after start.
fn run_with_sync(dir: &TempDir, peer_file: Option<PathBuf>) {
  let capture = script(&dir.0, "capture", &[Step::Sleep(3), Step::Exit(0)]);
  let index = script(&dir.0, "index", &[Step::Sleep(20)]);
  let mut config = config(dir, capture, index);
  let mut sync =
    script(&dir.0, "sync", &[Step::SayArgsAnd("LOGGER_PEER_TOKEN")]);
  sync.args = vec!["sync".into()];
  config.sync = sync;
  config.peer_file = peer_file;
  config.sync_first = Duration::from_millis(100);
  let exit = run(&config, &Stop::new()).unwrap();
  assert_eq!(exit, Exit::HandedOff);
}

#[test]
fn syncs_from_the_peer_file() {
  let dir = TempDir::new("sync");
  let peer_file = dir.0.join("peer.env");
  fs::write(
    &peer_file,
    "# the old host\nLOGGER_PEER_URL=http://peer:8788\n\
     LOGGER_PEER_TOKEN=\"secret\"\n",
  )
  .unwrap();
  run_with_sync(&dir, Some(peer_file));
  let sync = dir.log("sync");
  // The URL as an argument; the token only in its environment.
  assert!(
    sync.contains(" sync --peer http://peer:8788 secret"),
    "{sync}"
  );
  let supervisor = dir.log("supervisor");
  assert!(
    supervisor.contains("sync: from http://peer:8788"),
    "{supervisor}"
  );
  assert!(!supervisor.contains("secret"), "{supervisor}");
}

#[test]
fn skips_sync_without_a_peer() {
  let dir = TempDir::new("no-peer");
  let peer_file = dir.0.join("peer.env");
  run_with_sync(&dir, Some(peer_file));
  assert!(dir.log("sync").is_empty());
  assert!(dir.log("supervisor").contains("sync: no peer file at"));
  // Without a peer file configured, no sync at all.
  let dir = TempDir::new("no-peer-file");
  run_with_sync(&dir, None);
  assert!(!dir.0.join("logs").join("sync.log").exists());
}
