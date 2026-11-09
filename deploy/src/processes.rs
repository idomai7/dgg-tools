//! Finding and stopping the logger's processes by where their binaries
//! are: an install's are all under its `bin` directory.

use {
  ::std::{
    io,
    path::Path,
    thread,
    time::{Duration, Instant},
  },
  ::sysinfo::{
    Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind,
  },
};

fn system() -> io::Result<System> {
  let mut system =
    System::new().map_err(|e| io::Error::other(format!("processes: {e}")))?;
  system.refresh_processes_specifics(
    ProcessesToUpdate::All,
    true,
    ProcessRefreshKind::nothing()
      .with_exe(UpdateKind::Always)
      .with_cmd(UpdateKind::Always),
  );
  Ok(system)
}

/// Whether `process` runs one of `names` (without `.exe`) from under `dir`.
fn runs_from(process: &Process, dir: &Path, names: &[&str]) -> bool {
  process.exe().is_some_and(|exe| {
    exe.starts_with(dir)
      && exe
        .file_stem()
        .is_some_and(|stem| names.iter().any(|name| stem == *name))
  })
}

/// How many processes run one of `names` from under `dir`.
pub fn count_from(dir: &Path, names: &[&str]) -> io::Result<usize> {
  Ok(
    system()?
      .processes()
      .values()
      .filter(|process| runs_from(process, dir, names))
      .count(),
  )
}

/// Kills every process running one of `names` from under `dir`, and waits
/// (briefly) for them to go.
pub fn kill_from(dir: &Path, names: &[&str]) -> io::Result<()> {
  for process in system()?.processes().values() {
    if runs_from(process, dir, names) {
      process.kill();
    }
  }
  wait_gone(dir, names, Duration::from_secs(10))?;
  Ok(())
}

/// Waits up to `patience` for every process running one of `names` from
/// under `dir` to exit; whether they did.
pub fn wait_gone(
  dir: &Path,
  names: &[&str],
  patience: Duration,
) -> io::Result<bool> {
  let until = Instant::now() + patience;
  loop {
    if count_from(dir, names)? == 0 {
      return Ok(true);
    }
    if Instant::now() >= until {
      return Ok(false);
    }
    thread::sleep(Duration::from_millis(500));
  }
}

/// Kills every PowerShell running `script` from under `dir` for one of
/// `names`: one whose command line also names `<name>.exe`.
pub fn kill_scripts(
  dir: &Path,
  script: &str,
  names: &[&str],
) -> io::Result<()> {
  let dir = dir.to_string_lossy().to_lowercase();
  let script = script.to_lowercase();
  for process in system()?.processes().values() {
    let powershell = process.name().eq_ignore_ascii_case("powershell.exe");
    let line = process
      .cmd()
      .iter()
      .map(|arg| arg.to_string_lossy().to_lowercase())
      .collect::<Vec<_>>()
      .join(" ");
    let named = names
      .iter()
      .any(|name| line.contains(&format!("\\{}.exe", name.to_lowercase())));
    if powershell && named && line.contains(&dir) && line.contains(&script) {
      process.kill();
    }
  }
  Ok(())
}

/// How many processes run from anywhere under `dir`.
pub fn count_under(dir: &Path) -> io::Result<usize> {
  Ok(
    system()?
      .processes()
      .values()
      .filter(|process| process.exe().is_some_and(|exe| exe.starts_with(dir)))
      .count(),
  )
}

/// Kills every process running from under `dir`, `first` names first (a
/// supervisor, whose programs then die with it), and waits (briefly) for
/// them to go.
pub fn kill_under(dir: &Path, first: &[&str]) -> io::Result<()> {
  kill_from(dir, first)?;
  for process in system()?.processes().values() {
    if process.exe().is_some_and(|exe| exe.starts_with(dir)) {
      process.kill();
    }
  }
  let until = Instant::now() + Duration::from_secs(10);
  while count_under(dir)? > 0 && Instant::now() < until {
    thread::sleep(Duration::from_millis(200));
  }
  Ok(())
}

/// Kills every process running from under `dir` but not from under
/// `except`, and waits (briefly) for them to go.
pub fn kill_under_except(dir: &Path, except: &Path) -> io::Result<()> {
  let doomed = |exe: &Path| exe.starts_with(dir) && !exe.starts_with(except);
  for process in system()?.processes().values() {
    if process.exe().is_some_and(doomed) {
      process.kill();
    }
  }
  let until = Instant::now() + Duration::from_secs(10);
  while Instant::now() < until {
    let left = system()?
      .processes()
      .values()
      .filter(|process| process.exe().is_some_and(doomed))
      .count();
    if left == 0 {
      break;
    }
    thread::sleep(Duration::from_millis(200));
  }
  Ok(())
}

/// Waits up to `patience` for every process running one of `names` from
/// under `dir`, but not from under `except`, to exit; whether they did.
pub fn wait_gone_except(
  dir: &Path,
  names: &[&str],
  except: &Path,
  patience: Duration,
) -> io::Result<bool> {
  let until = Instant::now() + patience;
  loop {
    let left = system()?
      .processes()
      .values()
      .filter(|process| {
        runs_from(process, dir, names)
          && !process.exe().is_some_and(|exe| exe.starts_with(except))
      })
      .count();
    if left == 0 {
      return Ok(true);
    }
    if Instant::now() >= until {
      return Ok(false);
    }
    thread::sleep(Duration::from_millis(500));
  }
}
