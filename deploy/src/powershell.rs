//! Running PowerShell commands, for what only it reads well: scheduled
//! tasks, services and owners.

use ::std::{
  io,
  process::{Command, Stdio},
};

/// Runs a PowerShell command, its output read as UTF-8; `None` if it fails.
pub(crate) fn run(command: &str) -> io::Result<Option<String>> {
  let output = Command::new("powershell.exe")
    .args(["-NoProfile", "-NonInteractive", "-Command"])
    .arg(format!(
      "[Console]::OutputEncoding = [Text.Encoding]::UTF8; {command}"
    ))
    .stdin(Stdio::null())
    .output()?;
  Ok(
    output
      .status
      .success()
      .then(|| String::from_utf8_lossy(&output.stdout).into_owned()),
  )
}
