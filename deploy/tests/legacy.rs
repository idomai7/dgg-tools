//! Finding and retiring the PowerShell-era install.

use {
  ::deploy::legacy::{FOLDER, Legacy, parse},
  ::std::path::PathBuf,
};

/// A user install's capture task, as `Export-ScheduledTask` gives it
/// (account and user name replaced).
const USER_TASK: &str = include_str!("fixtures/legacy-user-task.xml");

#[test]
fn reads_a_user_install() {
  let root = PathBuf::from(r"C:\Users\someone\AppData\Local\dgg-tools");
  assert_eq!(
    parse(FOLDER, USER_TASK),
    Some(Legacy {
      folder: FOLDER.into(),
      machine: false,
      root: root.clone(),
      data: root.join("data"),
      logs: root.join("logs"),
    })
  );
}

#[test]
fn reads_a_machine_install() {
  let xml = USER_TASK
    .replace("S-1-5-21-1000000000-1000000000-1000000000-1001", "S-1-5-18")
    .replace(r"C:\Users\someone\AppData\Local", r"C:\ProgramData")
    // A custom data directory.
    .replace(r"C:\ProgramData\dgg-tools\data", r"D:\chat");
  let legacy = parse(FOLDER, &xml).unwrap();
  assert!(legacy.machine);
  assert_eq!(legacy.root, PathBuf::from(r"C:\ProgramData\dgg-tools"));
  assert_eq!(legacy.data, PathBuf::from(r"D:\chat"));
  assert_eq!(legacy.logs, PathBuf::from(r"C:\ProgramData\dgg-tools\logs"));
}

#[test]
fn reads_escaped_arguments() {
  let escaped = USER_TASK.replace(
    &USER_TASK[USER_TASK.find("<Arguments>").unwrap()
      ..USER_TASK.find("</Arguments>").unwrap()],
    &USER_TASK[USER_TASK.find("<Arguments>").unwrap()
      ..USER_TASK.find("</Arguments>").unwrap()]
      .replace('"', "&quot;"),
  );
  assert_eq!(parse(FOLDER, &escaped), parse(FOLDER, USER_TASK));
}

#[test]
fn ignores_other_tasks() {
  let other = USER_TASK.replace("supervise.ps1", "something-else.ps1");
  assert_eq!(parse(FOLDER, &other), None);
  assert_eq!(parse(FOLDER, "<Task></Task>"), None);
}

/// Against Task Scheduler itself: dormant tasks shaped like the old
/// install's, in a folder of their own, found and then retired.
#[cfg(windows)]
#[test]
fn finds_and_retires_an_install() {
  use {
    ::deploy::legacy::{Retired, find, retire, stop_index},
    ::std::{
      fs,
      process::{Command, Stdio},
      time::Duration,
    },
  };

  let folder = format!(r"\dgg-tools-test-{}\", ::std::process::id());
  let root = ::std::env::temp_dir()
    .join(format!("deploy-legacy-{}", ::std::process::id()));
  let build = root.join("bin").join("20261109-1-abc");
  fs::create_dir_all(&build).unwrap();
  fs::write(build.join("supervise.ps1"), "").unwrap();
  let (data, logs) = (root.join("data"), root.join("logs"));

  // Registered the way install.ps1 did, but set to run in five years.
  let register = |name: &str| {
    let arguments = format!(
      "-NoProfile -File \"{0}\\supervise.ps1\" -Log \"{1}\\{name}.log\" \
       -Program \"{0}\\{name}.exe\" -Data \"{2}\"",
      build.display(),
      logs.display(),
      data.display()
    );
    let script = format!(
      "$action = New-ScheduledTaskAction -Execute powershell.exe \
       -Argument '{}'; \
       $trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddYears(5); \
       Register-ScheduledTask -TaskPath '{folder}' -TaskName {name} \
       -Action $action -Trigger $trigger | Out-Null",
      arguments.replace('\'', "''")
    );
    let status = Command::new("powershell.exe")
      .args(["-NoProfile", "-NonInteractive", "-Command", &script])
      .status()
      .unwrap();
    assert!(status.success());
  };
  for name in ["capture", "index", "compact"] {
    register(name);
  }
  let exists = |name: &str| {
    Command::new("schtasks.exe")
      .args(["/Query", "/TN", &format!("{folder}{name}")])
      .output()
      .unwrap()
      .status
      .success()
  };

  let legacy = find(&folder).unwrap().unwrap();
  assert_eq!(legacy.root, root);
  assert_eq!((&legacy.data, &legacy.logs), (&data, &logs));
  assert!(!legacy.machine);

  stop_index(&legacy).unwrap();
  assert!(!exists("index") && !exists("compact"));
  assert!(exists("capture"));

  // The old capture, which won't hand off, and a new build's, which may be
  // in the same `bin` and must be left alone. Both just sleep.
  let new_build = root.join("bin").join("20261109-2-def");
  fs::create_dir_all(&new_build).unwrap();
  let sleep = |exe: PathBuf| {
    fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
    Command::new(exe)
      .args(["-n", "60", "127.0.0.1"])
      .stdout(Stdio::null())
      .spawn()
      .unwrap()
  };
  let mut old = sleep(build.join("capture.exe"));
  let mut new = sleep(new_build.join("dgg-capture.exe"));

  let retired =
    retire(&legacy, Duration::from_secs(1), Some(&new_build)).unwrap();
  assert_eq!(
    retired,
    Retired {
      killed_capture: true,
      left: Vec::new(),
    }
  );
  assert!(old.try_wait().unwrap().is_some());
  assert!(new.try_wait().unwrap().is_none());
  assert!(!exists("capture"));
  assert_eq!(find(&folder).unwrap(), None);
  assert!(!build.exists());
  new.kill().unwrap();
  new.wait().unwrap();
  let _ = fs::remove_dir_all(&root);
}
