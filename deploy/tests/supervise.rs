//! The supervisor, with small scripts standing in for the logger's programs.

use {
  ::common::temp_dir::TempDir,
  ::deploy::supervise::{Backoff, Config, Exit, Program, Stop, run},
  ::std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
  },
};

fn log(dir: &TempDir, name: &str) -> String {
  fs::read_to_string(dir.path().join("logs").join(format!("{name}.log")))
    .unwrap_or_default()
}

/// What a script does, in either shell.
enum Step<'a> {
  Say(&'a str),
  Sleep(u32),
  /// Exits 0 if `marker` exists, else creates it and exits 1.
  FailOnce(&'a str),
  /// Says its arguments, then the first line of its stdin.
  SayArgsAndStdin,
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
        Step::SayArgsAndStdin => "set /p LINE=\r\necho %* %LINE%\r\n".into(),
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
        Step::SayArgsAndStdin => "read -r line\necho \"$@\" \"$line\"\n".into(),
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
  let quiet = script(dir.path(), "compact", &[Step::Exit(0)]);
  let mut config = Config::new(
    dir.path(),
    &dir.path().join("data"),
    &dir.path().join("logs"),
  );
  config.dgg_capture = capture;
  config.migrate = script(dir.path(), "migrate", &[Step::Exit(0)]);
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
  let dir = TempDir::new("deploy-supervise-handoff");
  let capture = script(
    dir.path(),
    "capture",
    &[Step::Say("capturing"), Step::Sleep(1), Step::Exit(0)],
  );
  let index = script(
    dir.path(),
    "index",
    &[
      Step::Say("indexing"),
      Step::Say("WARN slow"),
      Step::Sleep(20),
    ],
  );
  let started = Instant::now();
  let exit = run(&config(&dir, capture, index), &Stop::new()).unwrap();
  assert_eq!(exit, Exit::HandedOff);
  // The index is killed, not waited for.
  assert!(started.elapsed() < Duration::from_secs(10));
  // A line that doesn't say its level is information; one that does keeps
  // it.
  assert!(log(&dir, "dgg-capture").contains("Z INFO  capturing"));
  let index = log(&dir, "index");
  assert!(index.contains("Z INFO  indexing"), "{index}");
  assert!(index.contains("Z WARN  slow"), "{index}");
  let supervisor = log(&dir, "supervisor");
  assert!(supervisor.contains("INFO  capture handed off; stopping"));
  assert!(supervisor.contains("index: stopped"));
}

#[test]
fn restarts_programs_that_fail() {
  let dir = TempDir::new("deploy-supervise-restart");
  let capture = script(dir.path(), "capture", &[Step::FailOnce("failed")]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let exit = run(&config(&dir, capture, index), &Stop::new()).unwrap();
  assert_eq!(exit, Exit::HandedOff);
  let supervisor = log(&dir, "supervisor");
  assert!(supervisor.contains("ERROR dgg-capture: exited, exit code: 1"));
  assert!(supervisor.contains("WARN  dgg-capture: restarting in 0.05s"));
}

#[test]
fn stops_on_request() {
  let dir = TempDir::new("deploy-supervise-stop");
  let capture = script(dir.path(), "capture", &[Step::Sleep(20)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
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
  let supervisor = log(&dir, "supervisor");
  assert!(supervisor.contains("dgg-capture: stopped"));
  assert!(supervisor.contains("index: stopped"));
}

#[test]
fn one_index_at_a_time() {
  let dir = TempDir::new("deploy-supervise-lock");
  let first = config(
    &dir,
    script(dir.path(), "capture", &[Step::Sleep(20)]),
    script(
      dir.path(),
      "index",
      &[Step::Say("first index"), Step::Sleep(20)],
    ),
  );
  let mut second = first.clone();
  second.logs = dir.path().join("logs2");
  second.index = script(dir.path(), "index2", &[Step::Say("second index")]);
  let (stop_first, stop_second) = (Stop::new(), Stop::new());
  thread::scope(|scope| {
    scope.spawn(|| run(&first, &stop_first).unwrap());
    thread::sleep(Duration::from_millis(1000));
    scope.spawn(|| run(&second, &stop_second).unwrap());
    thread::sleep(Duration::from_millis(1500));
    let waiting =
      fs::read_to_string(dir.path().join("logs2/supervisor.log")).unwrap();
    assert!(waiting.contains("another supervisor has it"), "{waiting}");
    assert!(
      !dir.path().join("logs2/index.log").exists() || {
        !fs::read_to_string(dir.path().join("logs2/index.log"))
          .unwrap()
          .contains("second index")
      }
    );
    // Once the first stops, the second takes over the index.
    stop_first.request();
    thread::sleep(Duration::from_millis(2500));
    let index = fs::read_to_string(dir.path().join("logs2/index.log")).unwrap();
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
  assert_eq!(args(&config.dgg_capture), "dgg-capture --data data");
  assert_eq!(args(&config.migrate), "migrate --data data");
  assert_eq!(args(&config.index), "index --follow --data data");
  assert_eq!(args(&config.compact), "compact --data data");
  assert_eq!(args(&config.sync), "sync --data data");
  assert_eq!(args(&config.kick_capture), "kick-capture --data data");
  assert_eq!(config.peers, None);
  assert_eq!(config.kick_channels, None);
  assert_eq!(
    ::deploy::supervise::kick_channels(Some(Path::new("root"))),
    Some(Path::new("root").join("kick-channels"))
  );
}

/// Runs `run` with a capture that hands off after a few seconds, and a sync
/// that says how it was called, due soon after start.
fn run_with_sync(dir: &TempDir, peers: Option<PathBuf>) {
  let capture = script(dir.path(), "capture", &[Step::Sleep(3), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let mut config = config(dir, capture, index);
  let mut sync = script(dir.path(), "sync", &[Step::SayArgsAndStdin]);
  sync.args = vec!["sync".into()];
  config.sync = sync;
  config.peers = peers;
  config.sync_first = Duration::from_millis(100);
  let exit = run(&config, &Stop::new()).unwrap();
  assert_eq!(exit, Exit::HandedOff);
}

#[test]
fn syncs_from_the_peer_file() {
  let dir = TempDir::new("deploy-supervise-sync");
  fs::write(
    dir.path().join("install.env.json"),
    r#"{"peers": {"peer": {"url": "http://peer:8788", "token": "secret"}}}"#,
  )
  .unwrap();
  run_with_sync(&dir, Some(dir.to_path_buf()));
  let sync = log(&dir, "sync");
  // The URL as an argument; the token only in its config, on stdin. Each
  // line says which peer it's about.
  assert!(
    sync.contains(
      r#" peer: sync --peer http://peer:8788 --config-stdin {"token":"secret"}"#
    ),
    "{sync}"
  );
  let supervisor = log(&dir, "supervisor");
  assert!(
    supervisor.contains("sync: from peer (http://peer:8788)"),
    "{supervisor}"
  );
  assert!(!supervisor.contains("secret"), "{supervisor}");
}

#[test]
fn syncs_from_every_node_in_turn() {
  let dir = TempDir::new("deploy-supervise-mesh");
  fs::write(
    dir.path().join("nodes"),
    "a https://a:8788\nb https://b:8788\nme https://me:8788\npi push\n",
  )
  .unwrap();
  // With the mesh's CA, HTTPS peers trust it alone; others can't.
  fs::write(
    dir.path().join("install.env.json"),
    r#"{"ca": "the CA", "mesh": {"node": "me", "token": "mesh"},
        "peers": {"old": {"url": "http://old:8788", "token": "old"}}}"#,
  )
  .unwrap();
  run_with_sync(&dir, Some(dir.to_path_buf()));
  let sync = log(&dir, "sync");
  let at = |text: &str| sync.find(text).unwrap_or_else(|| panic!("{sync}"));
  let a = at(
    r#" a: sync --peer https://a:8788 --config-stdin {"ca":"the CA","token":"mesh"}"#,
  );
  let b = at(
    r#" b: sync --peer https://b:8788 --config-stdin {"ca":"the CA","token":"mesh"}"#,
  );
  assert!(a < b, "{sync}");
  at(r#" old: sync --peer http://old:8788 --config-stdin {"token":"old"}"#);
  // Not itself, and not a node that can't be reached.
  assert!(
    !sync.contains("me:8788") && !sync.contains(" pi: "),
    "{sync}"
  );
}

#[test]
fn keeps_the_last_config_it_could_read() {
  let dir = TempDir::new("deploy-supervise-broken");
  let path = dir.path().join("install.env.json");
  fs::write(
    &path,
    r#"{"peers": {"peer": {"url": "http://peer:8788", "token": "secret"}}}"#,
  )
  .unwrap();
  let capture = script(dir.path(), "capture", &[Step::Sleep(4), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let mut config = config(&dir, capture, index);
  let mut sync = script(dir.path(), "sync", &[Step::SayArgsAndStdin]);
  sync.args = vec!["sync".into()];
  config.sync = sync;
  config.peers = Some(dir.to_path_buf());
  config.sync_first = Duration::from_millis(100);
  config.sync_every = Duration::from_millis(1500);
  // Broken after the first round.
  let breaking = thread::spawn(move || {
    thread::sleep(Duration::from_millis(800));
    fs::write(path, "{").unwrap();
  });
  assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  breaking.join().unwrap();
  let sync = log(&dir, "sync");
  let rounds = sync.matches(r#"{"token":"secret"}"#).count();
  assert!(rounds >= 2, "{sync}");
  let supervisor = log(&dir, "supervisor");
  assert!(supervisor.contains("syncing as before"), "{supervisor}");
  assert!(!supervisor.contains("secret"), "{supervisor}");
}

#[test]
fn serves_given_a_serve_section() {
  let dir = TempDir::new("deploy-supervise-serve");
  fs::write(
    dir.path().join("install.env.json"),
    r#"{"mesh": {"node": "me", "token": "a-made-up-mesh-token-for-tests-x"},
        "serve": {"listen": "127.0.0.1:8788"}}"#,
  )
  .unwrap();
  let capture = script(dir.path(), "capture", &[Step::Sleep(2), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let mut config = config(&dir, capture, index);
  let mut serve = script(
    dir.path(),
    "serve",
    &[Step::SayArgsAndStdin, Step::Sleep(20)],
  );
  serve.args = vec!["serve".into()];
  config.serve = serve;
  config.peers = Some(dir.to_path_buf());
  assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  // Where it listens as arguments; its tokens only on stdin, the read
  // token the mesh's.
  let serve = log(&dir, "serve");
  assert!(
    serve.contains(
      r#"serve --listen 127.0.0.1:8788 --config-stdin {"tokens":{"control":null,"query":null,"read":"a-made-up-mesh-token-for-tests-x"}}"#
    ),
    "{serve}"
  );
  let supervisor = log(&dir, "supervisor");
  assert!(!supervisor.contains("made-up"), "{supervisor}");

  // Without a serve section, nothing is served.
  let dir = TempDir::new("deploy-supervise-no-serve");
  let capture = script(dir.path(), "capture", &[Step::Sleep(1), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let mut quiet = self::config(&dir, capture, index);
  quiet.peers = Some(dir.to_path_buf());
  assert_eq!(run(&quiet, &Stop::new()).unwrap(), Exit::HandedOff);
  assert!(!dir.path().join("logs").join("serve.log").exists());
}

#[test]
fn skips_sync_without_a_peer() {
  let dir = TempDir::new("deploy-supervise-no-peer");
  run_with_sync(&dir, Some(dir.to_path_buf()));
  assert!(log(&dir, "sync").is_empty());
  assert!(log(&dir, "supervisor").contains("sync: no peers in"));
  // With nowhere to look for peers, no sync at all.
  let dir = TempDir::new("deploy-supervise-no-peer-file");
  run_with_sync(&dir, None);
  assert!(!dir.path().join("logs").join("sync.log").exists());
}

/// Where `line` first is in the supervisor's log.
fn said(dir: &TempDir, line: &str) -> usize {
  let log = log(dir, "supervisor");
  log
    .find(line)
    .unwrap_or_else(|| panic!("{line:?} not in:\n{log}"))
}

#[test]
fn indexes_once_migrated() {
  let dir = TempDir::new("deploy-supervise-migrate");
  let capture = script(
    dir.path(),
    "capture",
    &[Step::Say("capturing"), Step::Sleep(4), Step::Exit(0)],
  );
  let index = script(dir.path(), "index", &[Step::Say("indexing")]);
  let mut config = config(&dir, capture, index);
  config.migrate = script(
    dir.path(),
    "migrate",
    &[Step::Say("migrating"), Step::Sleep(1), Step::Exit(0)],
  );
  run(&config, &Stop::new()).unwrap();
  // Capture doesn't wait for it; the index does.
  assert!(said(&dir, "dgg-capture: started") < said(&dir, "migrate: exited"));
  assert!(said(&dir, "migrate: exited") < said(&dir, "index: started"));
  assert!(log(&dir, "migrate").contains(" migrating"));
  assert!(log(&dir, "index").contains(" indexing"));
}

#[test]
fn a_failed_migration_leaves_index_compact_and_sync_off() {
  let dir = TempDir::new("deploy-supervise-migrate");
  let capture = script(
    dir.path(),
    "capture",
    &[Step::Say("capturing"), Step::Sleep(2), Step::Exit(0)],
  );
  let index = script(dir.path(), "index", &[Step::Say("indexing")]);
  let mut config = config(&dir, capture, index);
  config.migrate = script(dir.path(), "migrate", &[Step::Exit(1)]);
  config.compact_first = Duration::from_millis(10);
  run(&config, &Stop::new()).unwrap();
  said(&dir, "migrate failed");
  assert!(log(&dir, "dgg-capture").contains(" capturing"));
  assert!(!log(&dir, "supervisor").contains("index: started"));
  assert!(!log(&dir, "supervisor").contains("compact: started"));
}

/// `config` with `kick` as Kick's capture, on a list holding `list`.
fn with_kick(
  dir: &TempDir,
  mut config: Config,
  kick: Program,
  list: &str,
) -> Config {
  let path = dir.path().join("kick-channels");
  fs::write(&path, list).unwrap();
  config.kick_capture = kick;
  config.kick_channels = Some(path);
  config.kick_check_every = Duration::from_millis(100);
  config.kick_handoff_wait = Duration::from_millis(300);
  config
}

#[test]
fn captures_kick_given_channels() {
  let dir = TempDir::new("deploy-supervise-kick");
  let capture = script(dir.path(), "capture", &[Step::Sleep(2), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let mut kick = script(
    dir.path(),
    "kick",
    &[Step::SayArgsAndStdin, Step::Sleep(20)],
  );
  kick.args = vec!["kick-capture".into()];
  let config = config(&dir, capture, index);
  let config = with_kick(&dir, config, kick, "one 2 1\n");
  let started = Instant::now();
  assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  assert!(started.elapsed() < Duration::from_secs(10));
  let list = dir.path().join("kick-channels");
  let said =
    format!("kick-capture --channels {} --channel one", list.display());
  assert!(log(&dir, "kick-capture.one").contains(&said));
  // Given a while to hand off too, then stopped.
  let supervisor = log(&dir, "supervisor");
  assert!(
    supervisor.contains("waiting up to 0.3s for kick-capture to hand off")
  );
  assert!(supervisor.contains("kick-capture one: stopped"));
}

#[test]
fn lets_kick_capture_hand_off_on_its_own() {
  let dir = TempDir::new("deploy-supervise-kick-handoff");
  let capture = script(dir.path(), "capture", &[Step::Sleep(1), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let kick = script(dir.path(), "kick", &[Step::Sleep(3), Step::Exit(0)]);
  let mut config = with_kick(&dir, config(&dir, capture, index), kick, "a 2 1");
  config.kick_handoff_wait = Duration::from_secs(30);
  let started = Instant::now();
  assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  assert!(started.elapsed() < Duration::from_secs(20));
  let supervisor = log(&dir, "supervisor");
  assert!(supervisor.contains("kick-capture a: ended"), "{supervisor}");
  assert!(
    !supervisor.contains("kick-capture a: stopped"),
    "{supervisor}"
  );
}

#[test]
fn waits_for_kick_channels() {
  let dir = TempDir::new("deploy-supervise-kick-none");
  let capture = script(dir.path(), "capture", &[Step::Sleep(1), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let kick = script(dir.path(), "kick", &[Step::Say("capturing kick")]);
  let config = with_kick(&dir, config(&dir, capture, index), kick, "# none\n");
  assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  let supervisor = log(&dir, "supervisor");
  // Said once, though looked for again and again.
  let none = supervisor.matches("kick-capture: no channels in").count();
  assert_eq!(none, 1, "{supervisor}");
  assert!(!supervisor.contains("kick-capture "), "{supervisor}");
  // No wait for a capture that never started.
  assert!(!supervisor.contains("waiting up to"));
}

#[test]
fn runs_a_kick_capture_per_channel() {
  let dir = TempDir::new("deploy-supervise-kick-each");
  let capture = script(dir.path(), "capture", &[Step::Sleep(2), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let mut kick = script(
    dir.path(),
    "kick",
    &[Step::SayArgsAndStdin, Step::Sleep(20)],
  );
  kick.args = vec!["kick-capture".into()];
  let config = with_kick(&dir, config(&dir, capture, index), kick, "one 2 1\n");
  let list = dir.path().join("kick-channels");
  thread::scope(|scope| {
    // A channel added while it runs.
    scope.spawn(|| {
      thread::sleep(Duration::from_millis(500));
      fs::write(&list, "one 2 1\ntwo 4 3\n").unwrap();
    });
    assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  });
  // Each to its own log.
  for slug in ["one", "two"] {
    let kick = log(&dir, &format!("kick-capture.{slug}"));
    assert!(kick.contains(&format!("--channel {slug}")), "{kick}");
    assert_eq!(kick.matches("--channel ").count(), 1, "{kick}");
  }
  let supervisor = log(&dir, "supervisor");
  for slug in ["one", "two"] {
    let started = format!("kick-capture {slug}: started");
    assert!(supervisor.contains(&started), "{supervisor}");
  }
}

#[test]
fn starts_an_ended_kick_capture_once_its_channel_is_back() {
  let dir = TempDir::new("deploy-supervise-kick-back");
  let capture = script(dir.path(), "capture", &[Step::Sleep(3), Step::Exit(0)]);
  let index = script(dir.path(), "index", &[Step::Sleep(20)]);
  let kick = script(dir.path(), "kick", &[Step::Exit(0)]);
  let config = with_kick(&dir, config(&dir, capture, index), kick, "a 2 1\n");
  let list = dir.path().join("kick-channels");
  thread::scope(|scope| {
    scope.spawn(|| {
      thread::sleep(Duration::from_millis(1000));
      fs::write(&list, "# none\n").unwrap();
      thread::sleep(Duration::from_millis(500));
      fs::write(&list, "a 2 1\n").unwrap();
    });
    assert_eq!(run(&config, &Stop::new()).unwrap(), Exit::HandedOff);
  });
  let supervisor = log(&dir, "supervisor");
  // Not again while listed, but again once back.
  let started = supervisor.matches("kick-capture a: started").count();
  assert_eq!(started, 2, "{supervisor}");
}
