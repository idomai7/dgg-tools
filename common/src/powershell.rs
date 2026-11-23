//! Running PowerShell commands, for what only it reads well on Windows,
//! such as scheduled tasks, services and file owners. Elsewhere there's no
//! `powershell.exe`, so [`run`] fails.

use ::std::{
  io,
  process::{Command, Stdio},
};

/// Runs a PowerShell command, its output read as UTF-8; `None` if it fails.
pub fn run(command: &str) -> io::Result<Option<String>> {
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
