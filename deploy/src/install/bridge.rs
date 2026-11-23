//! A bridge capture: started by an install straight from the new build,
//! capturing into the new data directory while the old install stops and
//! the new one starts, so no message is missed in between.
//!
//! It's an ordinary capture, so it hands off by itself: the old capture (if
//! it shares the data directory) retires in its favour, and it retires in
//! favour of the new install's capture, which starts after it. It runs
//! detached, appending to `capture.log` (its lines timestamped and marked
//! `(bridge)`), so it outlives the install if something goes wrong: capture
//! carries on until someone looks.
//!
//! Kick's capture gets a bridge of its own the same way, appending to
//! `kick-capture.log`, when the install has Kick channels.

use {
  super::exe,
  ::std::{
    ffi::OsStr,
    fs::OpenOptions,
    io,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
  },
};

/// Starts `build`'s capture against `data`, appending its output to `log`.
pub fn start(build: &Path, data: &Path, log: &Path) -> io::Result<Child> {
  spawn(
    build,
    &["capture".as_ref(), "--data".as_ref(), data.as_ref()],
    log,
  )
}

/// Starts `build`'s Kick capture against `data`, of the channels in
/// `list`, appending its output to `log`.
pub fn start_kick(
  build: &Path,
  data: &Path,
  list: &Path,
  log: &Path,
) -> io::Result<Child> {
  let args: [&OsStr; 5] = [
    "kick-capture".as_ref(),
    "--data".as_ref(),
    data.as_ref(),
    "--channels".as_ref(),
    list.as_ref(),
  ];
  spawn(build, &args, log)
}

/// Starts `build`'s `dgg-logger` with `args` and `--bridge`, detached,
/// appending its output to `log`.
fn spawn(build: &Path, args: &[&OsStr], log: &Path) -> io::Result<Child> {
  let output = OpenOptions::new().create(true).append(true).open(log)?;
  let spawn = |breakaway: bool| {
    let mut command = Command::new(exe(build, "dgg-logger"));
    command
      .args(args)
      .arg("--bridge")
      .stdin(Stdio::null())
      .stdout(output.try_clone()?)
      .stderr(output.try_clone()?);
    #[cfg(windows)]
    {
      use ::std::os::windows::process::CommandExt;
      const CREATE_NO_WINDOW: u32 = 0x0800_0000;
      const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
      const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
      let mut flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
      if breakaway {
        // Out of any job this install runs in (a terminal's, say), which
        // could kill it when the install exits.
        flags |= CREATE_BREAKAWAY_FROM_JOB;
      }
      command.creation_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = breakaway;
    command.spawn()
  };
  // A job that doesn't allow breaking away refuses the first.
  spawn(true).or_else(|_| spawn(false))
}

/// Waits up to `patience` for the bridge to hand off and exit; kills it
/// after that. Whether it left by itself.
pub fn wait_gone(bridge: &mut Child, patience: Duration) -> bool {
  let until = Instant::now() + patience;
  while Instant::now() < until {
    if !matches!(bridge.try_wait(), Ok(None)) {
      return true;
    }
    thread::sleep(Duration::from_millis(500));
  }
  let _ = bridge.kill();
  let _ = bridge.wait();
  false
}
