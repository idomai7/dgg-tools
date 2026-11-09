//! The machine install's Windows service: `dgg-logger`, started with the
//! system, running `dgg-supervisor --service` as its own virtual account
//! (`NT SERVICE\dgg-logger`) rather than SYSTEM. The account can write the
//! data and logs directories and read the binaries; nothing else.
//!
//! Managed with `sc.exe`, and read back through PowerShell (UTF-8, and
//! state names that aren't translated).

use {
  super::task::{Task, arguments, parse_arguments},
  crate::powershell,
  ::std::{
    io,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
  },
};

/// The service's name.
pub const NAME: &str = "dgg-logger";

/// The virtual account a service called `name` runs as.
pub fn account(name: &str) -> String {
  format!(r"NT SERVICE\{name}")
}

/// What the service runs, as one command line.
pub fn command_line(task: &Task) -> String {
  format!(
    "\"{}\" --service {}",
    task.supervisor.to_string_lossy(),
    arguments(task)
  )
}

/// Reads a service's command line back: `None` if it isn't one of ours.
pub fn parse(command_line: &str) -> Option<Task> {
  let rest = command_line.trim().strip_prefix('"')?;
  let (exe, rest) = rest.split_once('"')?;
  let supervisor = PathBuf::from(exe);
  let named = supervisor
    .file_stem()
    .is_some_and(|stem| stem == "dgg-supervisor");
  let rest = rest.trim_start().strip_prefix("--service")?;
  named.then(|| parse_arguments(supervisor, rest)).flatten()
}

fn quote(text: &str) -> String {
  format!("'{}'", text.replace('\'', "''"))
}

/// The service `name`, if it's installed.
pub fn find(name: &str) -> io::Result<Option<Task>> {
  let path = powershell::run(&format!(
    "(Get-CimInstance Win32_Service -Filter \"Name = {}\").PathName",
    quote(name)
  ))?;
  Ok(path.and_then(|path| parse(path.trim())))
}

/// Its state (`Running`, `Stopped`, ...), if it's installed.
pub fn state(name: &str) -> io::Result<Option<String>> {
  let state = powershell::run(&format!(
    "(Get-Service -Name {} -ErrorAction Stop).Status.ToString()",
    quote(name)
  ))?;
  Ok(
    state
      .map(|state| state.trim().to_owned())
      .filter(|s| !s.is_empty()),
  )
}

fn sc(args: &[&str]) -> io::Result<()> {
  let output = Command::new("sc.exe")
    .args(args)
    .stdin(Stdio::null())
    .output()?;
  if output.status.success() {
    Ok(())
  } else {
    Err(io::Error::other(format!(
      "sc {}: {}",
      args.join(" "),
      String::from_utf8_lossy(&output.stdout).trim()
    )))
  }
}

/// Creates the service `name` to run `task`, or points it at `task` if it
/// exists. Doesn't start it.
pub fn configure(name: &str, task: &Task) -> io::Result<()> {
  let command = command_line(task);
  if state(name)?.is_some() {
    sc(&["config", name, "binPath=", &command])?;
  } else {
    sc(&[
      "create",
      name,
      "binPath=",
      &command,
      "start=",
      "delayed-auto",
      "obj=",
      &account(name),
      "DisplayName=",
      "dgg-tools logger",
    ])?;
  }
  sc(&[
    "description",
    name,
    "Keeps the dgg-tools chat logger running.",
  ])?;
  // Restarted a minute after failing, however often; the count resets
  // after a day.
  sc(&[
    "failure",
    name,
    "reset=",
    "86400",
    "actions=",
    "restart/60000/restart/60000/restart/60000",
  ])
}

pub fn start(name: &str) -> io::Result<()> {
  sc(&["start", name])
}

/// Stops it (if it's running) and waits up to `patience` for it to stop.
pub fn stop(name: &str, patience: Duration) -> io::Result<bool> {
  if state(name)?.as_deref() != Some("Stopped") {
    // Already stopping is fine.
    let _ = sc(&["stop", name]);
  }
  let until = Instant::now() + patience;
  loop {
    match state(name)?.as_deref() {
      None | Some("Stopped") => return Ok(true),
      _ if Instant::now() >= until => return Ok(false),
      _ => thread::sleep(Duration::from_millis(500)),
    }
  }
}

/// Stops and deletes it, if it's installed.
pub fn delete(name: &str) -> io::Result<()> {
  if state(name)?.is_none() {
    return Ok(());
  }
  stop(name, Duration::from_secs(30))?;
  sc(&["delete", name])
}
