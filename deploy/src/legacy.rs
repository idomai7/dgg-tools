//! The install made by the PowerShell scripts that came before this crate:
//! three scheduled tasks, `capture`, `index` and `compact`, in a `\dgg-tools\`
//! folder, each running `supervise.ps1` on a binary in `<root>\bin\<build>\`.
//!
//! A new install takes over from one in two steps, so no message is missed:
//!
//! 1. [`stop_index`], before the new supervisor starts: the old index and
//!    compaction stop (they'd otherwise run beside the new ones) and their
//!    tasks go.
//! 2. [`retire`], once the new capture is healthy: the old capture hands off
//!    to it by itself and exits; then its task, the task folder and the old
//!    builds go.
//!
//! Scheduled tasks are read through PowerShell's `Export-ScheduledTask` (as
//! UTF-8; `schtasks /XML` uses the console's code page) and removed with
//! `schtasks`.

use {
  crate::processes,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
  },
};

/// The Task Scheduler folder the PowerShell install used.
pub const FOLDER: &str = r"\dgg-tools\";

/// One PowerShell-era install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Legacy {
  /// The Task Scheduler folder its tasks are in, like [`FOLDER`].
  pub folder: String,
  /// Run as SYSTEM (`-Scope Machine`) rather than as a user.
  pub machine: bool,
  /// Holds `bin\` and, by default, `data\` and `logs\`.
  pub root: PathBuf,
  pub data: PathBuf,
  pub logs: PathBuf,
}

/// Reads the install from its capture task's XML: `None` if it isn't one.
pub fn parse(folder: &str, xml: &str) -> Option<Legacy> {
  let arguments = unescape(between(xml, "<Arguments>", "</Arguments>")?);
  let user = between(xml, "<UserId>", "</UserId>")?;
  let field = |name: &str| -> Option<PathBuf> {
    Some(PathBuf::from(between(
      &arguments,
      &format!("-{name} \""),
      "\"",
    )?))
  };
  if !arguments.contains("supervise.ps1") {
    return None;
  }
  // <root>\bin\<build>\capture.exe
  let program = field("Program")?;
  let root = program.parent()?.parent()?.parent()?.to_owned();
  Some(Legacy {
    folder: folder.into(),
    machine: user == "S-1-5-18" || user.eq_ignore_ascii_case("SYSTEM"),
    root,
    data: field("Data")?,
    logs: field("Log")?.parent()?.to_owned(),
  })
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

/// Runs a PowerShell command, its output read as UTF-8; `None` if it fails.
pub(crate) fn powershell(command: &str) -> io::Result<Option<String>> {
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

/// Quotes `text` as a PowerShell string literal.
fn quote(text: &str) -> String {
  format!("'{}'", text.replace('\'', "''"))
}

/// The PowerShell-era install whose tasks are in `folder`, if there is one.
pub fn find(folder: &str) -> io::Result<Option<Legacy>> {
  let xml = powershell(&format!(
    "Export-ScheduledTask -TaskPath {} -TaskName capture",
    quote(folder)
  ))?;
  Ok(xml.and_then(|xml| parse(folder, &xml)))
}

fn task_exists(folder: &str, name: &str) -> io::Result<bool> {
  Ok(
    Command::new("schtasks.exe")
      .args(["/Query", "/TN", &format!("{folder}{name}")])
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()?
      .success(),
  )
}

/// Stops (`/End`) and deletes a task, if it exists.
fn delete_task(folder: &str, name: &str) -> io::Result<()> {
  if !task_exists(folder, name)? {
    return Ok(());
  }
  let task = format!("{folder}{name}");
  // Ending one that isn't running fails harmlessly.
  let _ = Command::new("schtasks.exe")
    .args(["/End", "/TN", &task])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
  let status = Command::new("schtasks.exe")
    .args(["/Delete", "/TN", &task, "/F"])
    .stdout(Stdio::null())
    .status()?;
  if status.success() {
    Ok(())
  } else {
    Err(io::Error::other(format!("schtasks couldn't delete {task}")))
  }
}

/// Step 1: stops the old index and compaction and removes their tasks.
pub fn stop_index(legacy: &Legacy) -> io::Result<()> {
  let bin = legacy.root.join("bin");
  // Builds from before the rename, and after.
  let names = ["index", "compact", "dgg-index", "dgg-compact"];
  delete_task(&legacy.folder, "index")?;
  delete_task(&legacy.folder, "compact")?;
  // Ending a task should stop its supervise.ps1, which may leave its
  // program running. Supervisors first, so neither is restarted.
  processes::kill_scripts(&bin, "supervise.ps1", &names)?;
  processes::kill_from(&bin, &names)?;
  Ok(())
}

/// What [`retire`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Retired {
  /// The old capture didn't hand off in time and was stopped.
  pub killed_capture: bool,
  /// Old builds that couldn't be deleted (still in use).
  pub left: Vec<PathBuf>,
}

/// Step 2, once a newer capture is healthy: waits up to `patience` for the
/// old capture to hand off and exit (stopping it after that: the new one is
/// running), then removes its task, the task folder and the old builds.
pub fn retire(legacy: &Legacy, patience: Duration) -> io::Result<Retired> {
  let bin = legacy.root.join("bin");
  let mut retired = Retired::default();
  let capture = ["capture", "dgg-capture"];
  if !processes::wait_gone(&bin, &capture, patience)? {
    // Its supervise.ps1 first, or it would restart it.
    processes::kill_scripts(&bin, "supervise.ps1", &capture)?;
    processes::kill_from(&bin, &capture)?;
    retired.killed_capture = true;
  }
  delete_task(&legacy.folder, "capture")?;
  // A clean exit ends supervise.ps1 too, but make sure.
  processes::kill_scripts(&bin, "supervise.ps1", &capture)?;
  let folder = legacy.folder.trim_matches('\\');
  powershell(&format!(
    "$service = New-Object -ComObject Schedule.Service; $service.Connect(); \
     $service.GetFolder('\\').DeleteFolder({}, 0)",
    quote(folder)
  ))?;
  retired.left = remove_builds(&bin);
  Ok(retired)
}

/// Deletes the PowerShell-era builds in `bin` (those with a supervise.ps1);
/// the ones that couldn't be.
fn remove_builds(bin: &Path) -> Vec<PathBuf> {
  let Ok(entries) = fs::read_dir(bin) else {
    return Vec::new();
  };
  entries
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .filter(|build| build.join("supervise.ps1").exists())
    .filter(|build| fs::remove_dir_all(build).is_err())
    .collect()
}
