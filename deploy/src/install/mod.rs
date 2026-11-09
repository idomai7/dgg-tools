//! Installing, upgrading and removing the logger for this user.
//!
//! An install is a root holding `bin\<build>\` (one directory per install,
//! so a running binary is never overwritten), `data\` and `logs\`, and a
//! scheduled task ([`task`]) that starts `dgg-supervisor` at logon.
//!
//! Installing again is an upgrade, with nothing missed: the new supervisor
//! starts beside the old one, and once its capture is healthy the old
//! capture hands off and its supervisor leaves. An install made by the old
//! PowerShell scripts is taken over the same way ([`legacy`]).

#[cfg(windows)]
pub mod acl;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
pub mod task;

use {
  crate::{
    legacy::{self, Legacy},
    log::{now_ms, timestamp},
    paths::{Paths, user_root},
    processes,
  },
  ::logger::control,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
  },
};

/// What a build holds, all beside `dgg-logger` when it's run.
pub const BINARIES: [&str; 9] = [
  "dgg-supervisor",
  "dgg-logger",
  "dgg-capture",
  "dgg-index",
  "dgg-compact",
  "dgg-backfill",
  "dgg-gaps",
  "dgg-serve",
  "dgg-sync",
];

/// What's installed now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
  /// This crate's scheduled task.
  Task(Paths),
  /// The PowerShell scripts' tasks.
  Legacy(Legacy),
}

impl Installed {
  pub fn paths(&self) -> Paths {
    match self {
      Self::Task(paths) => paths.clone(),
      Self::Legacy(legacy) => Paths {
        root: legacy.root.clone(),
        data: legacy.data.clone(),
        logs: legacy.logs.clone(),
      },
    }
  }
}

/// The installed logger, if any: this crate's task first, then the
/// PowerShell scripts'.
pub fn installed() -> io::Result<Option<Installed>> {
  #[cfg(windows)]
  if let Some(task) = task::find(task::NAME)? {
    // <root>\bin\<build>\dgg-supervisor.exe
    let root = task
      .supervisor
      .ancestors()
      .nth(3)
      .map(Path::to_owned)
      .unwrap_or_default();
    return Ok(Some(Installed::Task(Paths {
      root,
      data: task.data,
      logs: task.logs,
    })));
  }
  #[cfg(windows)]
  if let Some(legacy) = legacy::find(legacy::FOLDER)? {
    return Ok(Some(Installed::Legacy(legacy)));
  }
  Ok(None)
}

#[derive(Clone, Debug)]
pub struct Options {
  pub root: Option<PathBuf>,
  pub data: Option<PathBuf>,
  pub logs: Option<PathBuf>,
  /// Builds kept in `bin`, the new one included.
  pub keep: usize,
  /// How long the new capture may take to become healthy.
  pub healthy_within: Duration,
  /// How long the old capture may take to hand off and exit.
  pub handoff_within: Duration,
}

impl Default for Options {
  fn default() -> Self {
    Self {
      root: None,
      data: None,
      logs: None,
      keep: 3,
      healthy_within: Duration::from_secs(120),
      handoff_within: Duration::from_secs(120),
    }
  }
}

/// What [`install`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
  pub paths: Paths,
  pub build: PathBuf,
  /// What was installed before.
  pub replaced: Option<Installed>,
  /// The new capture instance.
  pub capture: String,
  /// Old builds still in use, left for next time.
  pub left: Vec<PathBuf>,
}

/// `YYYYMMDD-HHMMSS-<version>` for a build made now.
pub fn build_name(now: i64) -> String {
  let stamp: String = timestamp(now)
    .chars()
    .take(19)
    .filter(char::is_ascii_digit)
    .collect();
  format!(
    "{}-{}-{}",
    &stamp[..8],
    &stamp[8..],
    env!("CARGO_PKG_VERSION")
  )
}

/// Copies [`BINARIES`] from `from` into a new build directory in `bin`.
pub fn copy_build(from: &Path, bin: &Path, name: &str) -> io::Result<PathBuf> {
  let missing: Vec<_> = BINARIES
    .iter()
    .filter(|binary| !exe(from, binary).exists())
    .collect();
  if !missing.is_empty() {
    return Err(io::Error::other(format!(
      "{} isn't in {}: build everything first (cargo build --release)",
      missing
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join(", "),
      from.display()
    )));
  }
  let build = bin.join(name);
  fs::create_dir_all(&build)?;
  for binary in BINARIES {
    fs::copy(exe(from, binary), exe(&build, binary))?;
  }
  Ok(build)
}

fn exe(dir: &Path, name: &str) -> PathBuf {
  dir.join(format!("{name}{}", ::std::env::consts::EXE_SUFFIX))
}

/// Waits up to `patience` for a capture started at or after `since` (unix
/// ms) to be healthy; its instance.
pub fn wait_healthy(
  journal_root: &Path,
  since: i64,
  patience: Duration,
) -> io::Result<Option<String>> {
  let until = Instant::now() + patience;
  loop {
    let now = now_ms();
    let healthy = control::read_all(&control::dir(journal_root))?
      .into_iter()
      .find(|status| {
        status.started >= since
          && status.healthy_since.is_some()
          && now - status.heartbeat <= crate::status::STALE_MS
      });
    if let Some(status) = healthy {
      return Ok(Some(status.instance));
    }
    if Instant::now() >= until {
      return Ok(None);
    }
    thread::sleep(Duration::from_millis(500));
  }
}

/// Deletes all but the newest `keep` builds in `bin`, and any not in use:
/// the ones in use, which are left.
pub fn prune(bin: &Path, keep: usize) -> io::Result<Vec<PathBuf>> {
  let mut builds: Vec<PathBuf> = match fs::read_dir(bin) {
    Ok(entries) => entries
      .filter_map(Result::ok)
      .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
      .map(|entry| entry.path())
      .collect(),
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(e),
  };
  builds.sort();
  let old = builds.len().saturating_sub(keep);
  let mut left = Vec::new();
  for build in builds.into_iter().take(old) {
    if processes::count_under(&build)? > 0
      || fs::remove_dir_all(&build).is_err()
    {
      left.push(build);
    }
  }
  Ok(left)
}

/// Installs the build beside this program, or upgrades to it.
#[cfg(windows)]
pub fn install(options: &Options) -> io::Result<Done> {
  let replaced = installed()?;
  if let Some(Installed::Legacy(legacy)) = &replaced
    && legacy.machine
  {
    return Err(io::Error::other(
      "the installed logger is a machine install (install.ps1 -Scope \
       Machine); taking that over comes with the machine install",
    ));
  }
  let current = replaced.as_ref().map(Installed::paths);
  let root = options
    .root
    .clone()
    .or_else(|| current.as_ref().map(|paths| paths.root.clone()))
    .unwrap_or_else(user_root);
  let mut paths = Paths::under(root);
  if let Some(current) = &current {
    paths.data = current.data.clone();
    paths.logs = current.logs.clone();
  }
  paths.data = options.data.clone().unwrap_or(paths.data);
  paths.logs = options.logs.clone().unwrap_or(paths.logs);
  if let Some(current) = &current
    && *current != paths
  {
    return Err(io::Error::other(format!(
      "the installed logger uses {}, data {}, logs {}; moving it isn't \
       supported yet, so install with those (or uninstall first)",
      current.root.display(),
      current.data.display(),
      current.logs.display()
    )));
  }

  let from = ::std::env::current_exe()?
    .parent()
    .map(Path::to_owned)
    .ok_or_else(|| io::Error::other("can't tell where this program is"))?;
  let build = copy_build(&from, &paths.bin(), &build_name(now_ms()))?;
  fs::create_dir_all(&paths.data)?;
  fs::create_dir_all(&paths.logs)?;

  if let Some(Installed::Legacy(legacy)) = &replaced {
    legacy::stop_index(legacy)?;
  }
  task::register(
    task::NAME,
    &task::Task {
      supervisor: exe(&build, "dgg-supervisor"),
      data: paths.data.clone(),
      logs: paths.logs.clone(),
    },
  )?;
  // Statuses are written with this host's clock: a capture started from
  // now on is the new one.
  let since = now_ms() - 1000;
  task::run(task::NAME)?;
  let journal_root = paths.data.join("journal");
  let Some(capture) =
    wait_healthy(&journal_root, since, options.healthy_within)?
  else {
    return Err(io::Error::other(format!(
      "the new capture wasn't healthy within {}s; see supervisor.log and \
       capture.log in {}. Anything installed before is still running.",
      options.healthy_within.as_secs(),
      paths.logs.display()
    )));
  };

  let mut left = Vec::new();
  if let Some(Installed::Legacy(legacy)) = &replaced {
    left.extend(legacy::retire(legacy, options.handoff_within)?.left);
  }
  // The old supervisor leaves once its capture has handed off.
  let until = Instant::now() + options.handoff_within;
  while Instant::now() < until && older_running(&paths.bin(), &build)? {
    thread::sleep(Duration::from_millis(500));
  }
  left.extend(prune(&paths.bin(), options.keep)?);
  Ok(Done {
    paths,
    build,
    replaced,
    capture,
    left,
  })
}

/// Whether anything runs from a build in `bin` other than `current`.
fn older_running(bin: &Path, current: &Path) -> io::Result<bool> {
  let total = processes::count_under(bin)?;
  Ok(total > processes::count_under(current)?)
}

/// What [`uninstall`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
  pub paths: Option<Paths>,
  /// Left in place: the data and logs.
  pub kept: Vec<PathBuf>,
}

/// Removes the install (task, programs, binaries), keeping the data and
/// logs. `root` says where, if nothing is installed (to clean up after
/// shell runs).
#[cfg(windows)]
pub fn uninstall(root: Option<PathBuf>) -> io::Result<Removed> {
  let found = installed()?;
  if let Some(Installed::Legacy(legacy)) = &found
    && legacy.machine
  {
    return Err(io::Error::other(
      "the installed logger is a machine install (install.ps1 -Scope \
       Machine); remove it with deploy\\windows\\uninstall.ps1 -Scope Machine",
    ));
  }
  let paths = found
    .as_ref()
    .map(Installed::paths)
    .or_else(|| root.map(Paths::under));
  let Some(paths) = paths else {
    return Ok(Removed {
      paths: None,
      kept: Vec::new(),
    });
  };
  match &found {
    Some(Installed::Task(_)) => task::delete(task::NAME)?,
    Some(Installed::Legacy(legacy)) => {
      legacy::stop_index(legacy)?;
      legacy::retire(legacy, Duration::ZERO)?;
    }
    None => {}
  }
  let bin = paths.bin();
  processes::kill_under(&bin, &["dgg-supervisor"])?;
  if bin.exists() {
    fs::remove_dir_all(&bin)?;
  }
  let _ = fs::remove_dir(&paths.root);
  let kept = [&paths.data, &paths.logs]
    .into_iter()
    .filter(|path| path.exists())
    .cloned()
    .collect();
  Ok(Removed {
    paths: Some(paths),
    kept,
  })
}
