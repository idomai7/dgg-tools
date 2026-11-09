//! The user install's scheduled task: `\dgg-logger`, run at this user's
//! logon, starting `dgg-supervisor`.
//!
//! It's written as Task Scheduler XML and registered with `schtasks`, and
//! read back through PowerShell's `Export-ScheduledTask` (UTF-8, where
//! `schtasks /XML` uses the console's code page). Several instances may run
//! at once, so an upgrade's supervisor can start beside the old one, which
//! leaves once its capture has handed off.

use {
  crate::powershell,
  ::std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
  },
};

/// The task's name, in Task Scheduler's root folder.
pub const NAME: &str = "dgg-logger";

/// What the task runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
  /// `<root>\bin\<build>\dgg-supervisor.exe`.
  pub supervisor: PathBuf,
  pub data: PathBuf,
  pub logs: PathBuf,
  /// What it syncs from, if anything (`--peer-file`). Installs from before
  /// syncing have none.
  pub peer_file: Option<PathBuf>,
}

/// This user, as Task Scheduler names them.
pub fn user() -> String {
  let name = env::var("USERNAME").unwrap_or_default();
  match env::var("USERDOMAIN") {
    Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
    _ => name,
  }
}

fn escape(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}

/// `path` quoted for a command line. (A trailing backslash would escape the
/// closing quote, so it's dropped.)
fn quote(path: &Path) -> String {
  format!("\"{}\"", path.to_string_lossy().trim_end_matches('\\'))
}

/// The supervisor's arguments.
pub fn arguments(task: &Task) -> String {
  let mut arguments =
    format!("--data {} --logs {}", quote(&task.data), quote(&task.logs));
  if let Some(peer_file) = &task.peer_file {
    arguments += &format!(" --peer-file {}", quote(peer_file));
  }
  arguments
}

/// The task's XML, for `user`.
pub fn xml(task: &Task, user: &str) -> String {
  let user = escape(user);
  let command = escape(&task.supervisor.to_string_lossy());
  let arguments = escape(&arguments(task));
  let directory = task
    .supervisor
    .parent()
    .map(|dir| escape(&dir.to_string_lossy()))
    .unwrap_or_default();
  format!(
    r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Keeps the dgg-tools chat logger running.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
  )
}

/// The text between the first `open` and the `close` after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
  let start = text.find(open)? + open.len();
  let end = text[start..].find(close)? + start;
  Some(&text[start..end])
}

fn unescape(text: &str) -> String {
  text
    .replace("&quot;", "\"")
    .replace("&apos;", "'")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&amp;", "&")
}

/// Reads a task back from its XML: `None` if it isn't one of ours.
pub fn parse(xml: &str) -> Option<Task> {
  let command = unescape(between(xml, "<Command>", "</Command>")?);
  let arguments = unescape(between(xml, "<Arguments>", "</Arguments>")?);
  let supervisor = PathBuf::from(command.trim_matches('"'));
  let named = supervisor
    .file_stem()
    .is_some_and(|stem| stem == "dgg-supervisor");
  named
    .then(|| parse_arguments(supervisor, &arguments))
    .flatten()
}

/// `supervisor` run with `arguments` (as [`arguments`] writes them).
pub fn parse_arguments(supervisor: PathBuf, arguments: &str) -> Option<Task> {
  let field = |name: &str| -> Option<PathBuf> {
    Some(PathBuf::from(between(
      arguments,
      &format!("--{name} \""),
      "\"",
    )?))
  };
  Some(Task {
    data: field("data")?,
    logs: field("logs")?,
    peer_file: field("peer-file"),
    supervisor,
  })
}

/// The task `name` (normally [`NAME`]), if it's installed.
pub fn find(name: &str) -> io::Result<Option<Task>> {
  let xml = powershell::run(&format!(
    r"Export-ScheduledTask -TaskPath '\' -TaskName '{}'",
    name.replace('\'', "''")
  ))?;
  Ok(xml.and_then(|xml| parse(&xml)))
}

fn schtasks(args: &[&str]) -> io::Result<bool> {
  Ok(
    Command::new("schtasks.exe")
      .args(args)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()?
      .success(),
  )
}

/// Registers `task` for this user, replacing any earlier definition (which
/// leaves a running instance running).
pub fn register(name: &str, task: &Task) -> io::Result<()> {
  let path =
    env::temp_dir().join(format!("dgg-logger-{}.xml", ::std::process::id()));
  // Task Scheduler wants the UTF-16 it says it is.
  let mut bytes = vec![0xff, 0xfe];
  for unit in xml(task, &user()).encode_utf16() {
    bytes.extend_from_slice(&unit.to_le_bytes());
  }
  fs::write(&path, bytes)?;
  let path_text = path.to_string_lossy().into_owned();
  let registered =
    schtasks(&["/Create", "/TN", name, "/XML", &path_text, "/F"]);
  let _ = fs::remove_file(&path);
  if registered? {
    Ok(())
  } else {
    Err(io::Error::other(format!(
      "schtasks couldn't register {name}"
    )))
  }
}

/// Starts an instance now.
pub fn run(name: &str) -> io::Result<()> {
  if schtasks(&["/Run", "/TN", name])? {
    Ok(())
  } else {
    Err(io::Error::other(format!("schtasks couldn't start {name}")))
  }
}

/// Stops every instance and deletes the task, if it exists.
pub fn delete(name: &str) -> io::Result<()> {
  if !schtasks(&["/Query", "/TN", name])? {
    return Ok(());
  }
  // Ending one that isn't running fails harmlessly.
  let _ = schtasks(&["/End", "/TN", name]);
  if schtasks(&["/Delete", "/TN", name, "/F"])? {
    Ok(())
  } else {
    Err(io::Error::other(format!("schtasks couldn't delete {name}")))
  }
}
