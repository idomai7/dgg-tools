//! Installing, upgrading, moving and removing the logger.
//!
//! An install is a root holding `bin\<build>\` (one directory per install,
//! so a running binary is never overwritten), `data\` and `logs\`, and
//! something that keeps `dgg-supervisor` running:
//!
//! - a user install: a scheduled task started at this user's logon
//!   ([`task`]), in `%LOCALAPPDATA%\dgg-tools`;
//! - a machine install: a Windows service started with the system, running
//!   as its own account ([`service`]), in `C:\ProgramData\dgg-tools`, which
//!   only administrators may change ([`acl`]).
//!
//! Every install goes the same way, so that whatever was there before (an
//! older build, the other kind of install, the PowerShell scripts' install)
//! hands over without a message missed:
//!
//! 1. The new build is copied in, and a [`bridge`] capture from it started
//!    against the new data directory, and waited on until it's healthy.
//! 2. What was installed stops. If it shared the data directory, its capture
//!    hands off to the bridge first.
//! 3. If the data directory changed, the old data and logs are [moved]
//!    (moving) in.
//! 4. The new task or service starts. Its capture, the newest, takes over
//!    from the bridge, which exits.

#[cfg(windows)]
pub mod acl;
pub mod bridge;
pub mod moving;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
  /// A scheduled task, for this user.
  User,
  /// A Windows service, for the machine.
  Machine,
}

impl Scope {
  /// Where an install of this scope goes by default.
  pub fn default_root(self) -> PathBuf {
    match self {
      Self::User => user_root(),
      Self::Machine => ::std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("dgg-tools"),
    }
  }
}

/// What's installed now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
  /// A user install: this crate's scheduled task.
  Task(Paths),
  /// A machine install: this crate's service.
  Service(Paths),
  /// The PowerShell scripts' tasks.
  Legacy(Legacy),
}

impl Installed {
  pub fn paths(&self) -> Paths {
    match self {
      Self::Task(paths) | Self::Service(paths) => paths.clone(),
      Self::Legacy(legacy) => Paths {
        root: legacy.root.clone(),
        data: legacy.data.clone(),
        logs: legacy.logs.clone(),
      },
    }
  }

  pub fn scope(&self) -> Scope {
    match self {
      Self::Task(_) => Scope::User,
      Self::Service(_) => Scope::Machine,
      Self::Legacy(legacy) if legacy.machine => Scope::Machine,
      Self::Legacy(_) => Scope::User,
    }
  }
}

/// `<root>\bin\<build>\dgg-supervisor.exe` run against `data` and `logs`.
#[cfg(windows)]
fn paths_of(task: task::Task) -> Paths {
  let root = task
    .supervisor
    .ancestors()
    .nth(3)
    .map(Path::to_owned)
    .unwrap_or_default();
  Paths {
    root,
    data: task.data,
    logs: task.logs,
  }
}

/// The installed logger, if any: this crate's service, then its task, then
/// the PowerShell scripts' tasks.
pub fn installed() -> io::Result<Option<Installed>> {
  #[cfg(windows)]
  {
    if let Some(found) = service::find(service::NAME)? {
      return Ok(Some(Installed::Service(paths_of(found))));
    }
    if let Some(found) = task::find(task::NAME)? {
      return Ok(Some(Installed::Task(paths_of(found))));
    }
    if let Some(legacy) = legacy::find(legacy::FOLDER)? {
      return Ok(Some(Installed::Legacy(legacy)));
    }
  }
  Ok(None)
}

#[derive(Clone, Debug)]
pub struct Options {
  pub scope: Scope,
  pub root: Option<PathBuf>,
  pub data: Option<PathBuf>,
  pub logs: Option<PathBuf>,
  /// Builds kept in `bin`, the new one included.
  pub keep: usize,
  /// How long a new capture may take to become healthy.
  pub healthy_within: Duration,
  /// How long an old capture may take to hand off and exit.
  pub handoff_within: Duration,
}

impl Default for Options {
  fn default() -> Self {
    Self {
      scope: Scope::User,
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
  pub scope: Scope,
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
      .filter(|status| {
        status.started >= since
          && status.healthy_since.is_some()
          && now - status.heartbeat <= crate::status::STALE_MS
      })
      .max_by_key(|status| status.started);
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

/// The directories an install in `scope` would use, given `options` and
/// what's installed: an install of the same scope keeps its own unless told
/// otherwise; anything else gets the scope's defaults.
pub fn plan(options: &Options, current: Option<&Installed>) -> Paths {
  let same = current.filter(|current| current.scope() == options.scope);
  let kept = same.map(Installed::paths);
  let root = options
    .root
    .clone()
    .or_else(|| kept.as_ref().map(|paths| paths.root.clone()))
    .unwrap_or_else(|| options.scope.default_root());
  let mut paths = Paths::under(root);
  if let Some(kept) = kept.filter(|kept| kept.root == paths.root) {
    paths.data = kept.data;
    paths.logs = kept.logs;
  }
  paths.data = options.data.clone().unwrap_or(paths.data);
  paths.logs = options.logs.clone().unwrap_or(paths.logs);
  paths
}

fn error(message: impl Into<String>) -> io::Error {
  io::Error::other(message.into())
}

/// Installs the build beside this program, or upgrades or moves the
/// installed logger to it.
#[cfg(windows)]
pub fn install(options: &Options) -> io::Result<Done> {
  let current = installed()?;
  let machine = options.scope == Scope::Machine
    || current
      .as_ref()
      .is_some_and(|current| current.scope() == Scope::Machine);
  if machine && !acl::elevated() {
    return Err(error(
      "a machine install, or changing one, needs an elevated (administrator) \
       prompt",
    ));
  }
  let paths = plan(options, current.as_ref());
  let old = current.as_ref().map(Installed::paths);
  let moving = old
    .as_ref()
    .is_some_and(|old| !moving::same(&old.data, &paths.data));
  if moving && let Some(old) = &old {
    let collisions = moving::collisions(&old.data, &paths.data)?;
    if !collisions.is_empty() {
      return Err(error(format!(
        "{} and {} both have {}; move one aside, then try again",
        old.data.display(),
        paths.data.display(),
        collisions[0].display()
      )));
    }
  }
  let from = ::std::env::current_exe()?
    .parent()
    .map(Path::to_owned)
    .ok_or_else(|| error("can't tell where this program is"))?;

  // 1. The build, and a bridge capture from it.
  if options.scope == Scope::Machine {
    acl::protect(&paths.root)?;
  }
  let build = copy_build(&from, &paths.bin(), &build_name(now_ms()))?;
  fs::create_dir_all(&paths.data)?;
  fs::create_dir_all(&paths.logs)?;
  let launch = task::Task {
    supervisor: exe(&build, "dgg-supervisor"),
    data: paths.data.clone(),
    logs: paths.logs.clone(),
  };
  if options.scope == Scope::Machine {
    // Before the service's account can be granted anything, it must exist.
    service::configure(service::NAME, &launch)?;
    allow_service(&paths)?;
  }
  let journal_root = paths.data.join("journal");
  let since = now_ms() - 1000;
  let mut bridge =
    bridge::start(&build, &paths.data, &paths.logs.join("bridge.log"))?;
  if wait_healthy(&journal_root, since, options.healthy_within)?.is_none() {
    let _ = bridge.kill();
    return Err(error(format!(
      "a capture from the new build wasn't healthy within {}s; see \
       bridge.log in {}. Nothing else was changed.",
      options.healthy_within.as_secs(),
      paths.logs.display()
    )));
  }

  // 2. What was installed stops.
  if let Some(current) = &current {
    let old = current.paths();
    if !moving {
      // Sharing the data, its capture hands off to the bridge.
      processes::wait_gone_except(
        &old.bin(),
        &["dgg-capture", "capture"],
        &build,
        options.handoff_within,
      )?;
    }
    stop(current, options.scope, &build)?;

    // 3. Its data and logs move in.
    if moving {
      moving::merge(&old.data, &paths.data)?;
    }
    moving::merge_logs(&old.logs, &paths.logs)?;
    if !moving::same(&old.root, &paths.root) {
      remove_root(&old)?;
    }
    match options.scope {
      Scope::Machine => allow_service(&paths)?,
      Scope::User if current.scope() == Scope::Machine => {
        // Out of a machine install's protected directories.
        acl::release(&paths.data, &task::user())?;
        acl::release(&paths.logs, &task::user())?;
      }
      Scope::User => {}
    }
  }

  // 4. The new install starts, and takes over from the bridge.
  let since = now_ms() - 1000;
  match options.scope {
    Scope::User => {
      task::register(task::NAME, &launch)?;
      task::run(task::NAME)?;
    }
    Scope::Machine => service::start(service::NAME)?,
  }
  let Some(capture) =
    wait_healthy(&journal_root, since, options.healthy_within)?
  else {
    return Err(error(format!(
      "the new install's capture wasn't healthy within {}s; see \
       supervisor.log and capture.log in {}. The bridge capture keeps \
       capturing meanwhile.",
      options.healthy_within.as_secs(),
      paths.logs.display()
    )));
  };
  bridge::wait_gone(&mut bridge, options.handoff_within);
  let left = prune(&paths.bin(), options.keep)?;
  Ok(Done {
    scope: options.scope,
    paths,
    build,
    replaced: current,
    capture,
    left,
  })
}

/// A machine install's root protected, and its service's account allowed
/// to change the data and logs.
#[cfg(windows)]
fn allow_service(paths: &Paths) -> io::Result<()> {
  acl::protect(&paths.root)?;
  let account = service::account(service::NAME);
  for dir in [&paths.data, &paths.logs] {
    if !dir.starts_with(&paths.root) {
      acl::protect(dir)?;
    }
    acl::grant(dir, &account)?;
  }
  Ok(())
}

/// Stops `current` and everything it runs, except from `keep` (the new
/// build: the bridge). What the new install replaces in place (the same
/// task, the same service) is left registered.
#[cfg(windows)]
fn stop(current: &Installed, scope: Scope, keep: &Path) -> io::Result<()> {
  match current {
    Installed::Task(_) if scope == Scope::User => {}
    Installed::Task(_) => task::delete(task::NAME)?,
    Installed::Service(_) if scope == Scope::Machine => {
      service::stop(service::NAME, Duration::from_secs(60))?;
    }
    Installed::Service(_) => service::delete(service::NAME)?,
    Installed::Legacy(legacy) => {
      legacy::stop_index(legacy)?;
      legacy::retire(legacy, Duration::ZERO)?;
    }
  }
  processes::kill_under_except(&current.paths().bin(), keep)
}

/// Removes an old root's binaries, and the root if that leaves it empty.
fn remove_root(old: &Paths) -> io::Result<()> {
  let bin = old.bin();
  if bin.exists() {
    fs::remove_dir_all(&bin)?;
  }
  let _ = fs::remove_dir(&old.root);
  Ok(())
}

/// What [`uninstall`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
  pub paths: Option<Paths>,
  /// Left in place: the data and logs.
  pub kept: Vec<PathBuf>,
}

/// Removes the install (task or service, programs, binaries), keeping the
/// data and logs; a machine install's get ordinary permissions back, owned
/// by this user. `root` says where, if nothing is installed (to clean up
/// after shell runs).
#[cfg(windows)]
pub fn uninstall(root: Option<PathBuf>) -> io::Result<Removed> {
  let found = installed()?;
  let machine = found
    .as_ref()
    .is_some_and(|found| found.scope() == Scope::Machine);
  if machine && !acl::elevated() {
    return Err(error(
      "removing a machine install needs an elevated (administrator) prompt",
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
    Some(Installed::Service(_)) => service::delete(service::NAME)?,
    Some(Installed::Legacy(legacy)) => {
      legacy::stop_index(legacy)?;
      legacy::retire(legacy, Duration::ZERO)?;
    }
    None => {}
  }
  processes::kill_under(&paths.bin(), &["dgg-supervisor"])?;
  remove_root(&paths)?;
  if machine {
    let user = task::user();
    for dir in [&paths.data, &paths.logs, &paths.root] {
      acl::release(dir, &user)?;
    }
  }
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
