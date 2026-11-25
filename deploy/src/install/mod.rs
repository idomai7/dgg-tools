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
//! older build, or the other kind of install) hands over without a message
//! missed:
//!
//! 1. The new build is copied in, and a [`bridge`] capture from it started
//!    against the new data directory for each of the install's dgg
//!    channels, and waited on until it's healthy. So is a Kick one for each
//!    of its Kick channels. They're started [`STAGGER`] apart, dgg's
//!    first, so their handoffs are spread out too.
//! 2. What was installed stops. If it shared the data directory, its
//!    captures hand off to the bridges first.
//! 3. If the data directory changed, the old data and logs are
//!    [moved](moving) in.
//! 4. The new task or service starts. Its captures, the newest, take over
//!    from the bridges, which exit. Each is waited on until it's healthy.
//!
//! A bridge that isn't healthy in time is stopped and warned of, by its
//! channel, and the install carries on without it: once any dgg bridge has
//! taken over, what was installed stops everything it runs, so there's no
//! going back. Only if no dgg bridge is healthy does the install stop
//! there, what was installed running on (and any Kick bridge that took
//! over left capturing, until the next install takes over from it). A new
//! capture that isn't healthy in time is warned of too, its bridge left
//! capturing, to hand off once it is.
//!
//! An install syncs from the peers in its root (see [`crate::peer`]):
//! [`Options::peers`] changes them, in its config, `install.env.json`,
//! which an older install's files are folded into. They move with the
//! root, and in a machine install only administrators and the service can
//! read the config. The channel lists (see [`crate::lists`]) move with the
//! root too. A list that isn't there is written before the bridges start:
//! converted from the plain file it was before, if there's one, else new
//! (dgg's with destiny.gg alone). The plain file is renamed `<name>.old`
//! once what was installed has stopped, since it reads it.

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
    lists,
    paths::{Paths, user_root},
    peer, processes,
    supervise::{STAGGER, dgg_capture_log, kick_capture_log},
  },
  ::common::{
    fs::{MissingOk, same},
    time::{now_ms, utc_timestamp},
  },
  ::logger::{
    control,
    dgg::{self, Dgg},
    kick::{capture::control_dir, channels},
    service::Service,
  },
  ::logger_journal::{Variant, layout::kick_root},
  ::std::{
    fs, io,
    path::{Path, PathBuf},
    process::Child,
    thread,
    time::{Duration, Instant},
  },
};

/// What a build holds, both beside `dgg-logger` when it's run.
pub const BINARIES: [&str; 2] = ["dgg-supervisor", "dgg-logger"];

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
}

impl Installed {
  pub fn paths(&self) -> Paths {
    match self {
      Self::Task(paths) | Self::Service(paths) => paths.clone(),
    }
  }

  pub fn scope(&self) -> Scope {
    match self {
      Self::Task(_) => Scope::User,
      Self::Service(_) => Scope::Machine,
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

/// The installed logger, if any: the service, then the task.
pub fn installed() -> io::Result<Option<Installed>> {
  #[cfg(windows)]
  {
    if let Some(found) = service::find(service::NAME)? {
      return Ok(Some(Installed::Service(paths_of(found))));
    }
    if let Some(found) = task::find(task::NAME)? {
      return Ok(Some(Installed::Task(paths_of(found))));
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
  /// Changes to the peers it syncs from, in order. Whatever else is there
  /// is kept (and moved, if the root moves).
  pub peers: Vec<peer::Change>,
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
      peers: Vec::new(),
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
  /// Each dgg channel it captures, by its name, with its new capture
  /// instance if that was healthy in time.
  pub dgg: Vec<(String, Option<String>)>,
  /// Each Kick channel it captures, by its slug, with its new capture
  /// instance if that was healthy in time.
  pub kick: Vec<(String, Option<String>)>,
  /// Old builds still in use, left for next time.
  pub left: Vec<PathBuf>,
  /// The peers it syncs from: `<name> (<URL>)`.
  pub peers: Vec<String>,
  /// What about its peers couldn't be read, or went wrong with a
  /// channel's capture.
  pub problems: Vec<String>,
  /// What was done to the channel lists, a line each.
  pub lists: Vec<String>,
}

/// `YYYYMMDD-HHMMSS-<build>` for a build made now (see [`crate::BUILD`]).
pub fn build_name(now: i64) -> String {
  let stamp: String = utc_timestamp(now)
    .chars()
    .take(19)
    .filter(char::is_ascii_digit)
    .collect();
  format!("{}-{}-{}", &stamp[..8], &stamp[8..], crate::BUILD)
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

/// Waits up to `patience` for a capture of dgg channel `name` started at
/// or after `since` (unix ms) to be healthy, as its status in the journal
/// directory `journal` has it; its instance.
pub fn wait_healthy(
  journal: &Path,
  name: &str,
  since: i64,
  patience: Duration,
) -> io::Result<Option<String>> {
  let dir = control::dir(&Dgg::channel_dir(&Dgg::dir(journal), name));
  wait_healthy_in(&dir, Variant::Dgg, since, patience)
}

/// Stops `bridges`.
fn kill_bridges(bridges: &mut Vec<(String, Child)>) {
  for (_, mut child) in bridges.drain(..) {
    let _ = child.kill();
    let _ = child.wait();
  }
}

/// Waits up to `patience` for each of `bridges` whose channel's new
/// capture is healthy, as `captures` has it, to hand off to it, stopping
/// it after that (see [`bridge::wait_gone`]). One whose channel's isn't is
/// left capturing, to hand off once it is. What went wrong, a line each,
/// each bridge called a `what` bridge, its log named by `log`.
fn hand_off(
  bridges: Vec<(String, Child)>,
  captures: &[(String, Option<String>)],
  patience: Duration,
  what: &str,
  log: fn(&str) -> String,
) -> Vec<String> {
  let mut problems = Vec::new();
  for (channel, mut child) in bridges {
    let healthy = captures
      .iter()
      .any(|(name, capture)| *name == channel && capture.is_some());
    if !healthy {
      problems.push(format!(
        "{channel}: the {what}bridge is left capturing, to hand off once \
         the new install's capture is healthy"
      ));
    } else if !bridge::wait_gone(&mut child, patience) {
      problems.push(format!(
        "{channel}: the {what}bridge didn't hand off within {}s, and was \
         stopped; see {}.log",
        patience.as_secs(),
        log(&channel)
      ));
    }
  }
  problems
}

/// The log of dgg channel `name`'s capture, in `logs`.
fn dgg_log(logs: &Path, name: &str) -> PathBuf {
  logs.join(format!("{}.log", dgg_capture_log(name)))
}

/// [`wait_healthy`], for channel `slug`'s Kick capture: as its journal
/// root under Kick's, `kick_root`, has it.
pub fn wait_kick_healthy(
  kick_root: &Path,
  slug: &str,
  since: i64,
  patience: Duration,
) -> io::Result<Option<String>> {
  let dir = control_dir(kick_root, slug);
  wait_healthy_in(&dir, Variant::Kick, since, patience)
}

fn wait_healthy_in(
  dir: &Path,
  variant: Variant,
  since: i64,
  patience: Duration,
) -> io::Result<Option<String>> {
  let until = Instant::now() + patience;
  loop {
    let now = now_ms();
    let healthy = control::read_all_as(dir, variant)?
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
  let Some(entries) = fs::read_dir(bin).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut builds: Vec<PathBuf> = entries
    .filter_map(Result::ok)
    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
    .map(|entry| entry.path())
    .collect();
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
    .is_some_and(|old| !same(&old.data, &paths.data));
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
    // Protecting opened it to every user; until the service exists, only
    // administrators.
    secure_peers(&paths, &[])?;
  }
  let build = copy_build(&from, &paths.bin(), &build_name(now_ms()))?;
  fs::create_dir_all(&paths.data)?;
  fs::create_dir_all(&paths.logs)?;
  let launch = task::Task {
    supervisor: exe(&build, "dgg-supervisor"),
    data: paths.data.clone(),
    logs: paths.logs.clone(),
    peers: Some(paths.root.clone()),
  };
  if options.scope == Scope::Machine {
    // Before the service's account can be granted anything, it must exist.
    service::configure(service::NAME, &launch)?;
    allow_service(&paths)?;
  }
  // The channel lists move with the root.
  for list in lists::FILES {
    let to = paths.root.join(list);
    if let Some(old) = &old
      && !same(&old.root, &paths.root)
      && !to.exists()
    {
      let from = old.root.join(list);
      if from.exists() {
        fs::copy(&from, &to)?;
      }
    }
  }
  let mut lists_done = lists::write_missing(&paths.root).map_err(error)?;
  // Kick's channels, if Kick is captured.
  let kick_list = paths.root.join(channels::FILE);
  let kick_channels = channels::load(&kick_list).map_err(error)?;
  // dgg's, each of which must be captured.
  let dgg_list = paths.root.join(dgg::channels::FILE);
  let dgg_channels: Vec<String> = dgg::channels::load(&dgg_list)
    .map_err(error)?
    .into_iter()
    .map(|channel| channel.name)
    .collect();
  let mut problems = Vec::new();

  let journal_root = paths.data.join("journal");
  let since = now_ms() - 1000;
  // A bridge per dgg channel, by its name, then one per Kick channel, by
  // its slug, each a stagger after the last.
  let mut bridges = Vec::new();
  for (n, name) in dgg_channels.iter().enumerate() {
    if n > 0 {
      thread::sleep(STAGGER);
    }
    let log = dgg_log(&paths.logs, name);
    match bridge::start(&build, &paths.data, &dgg_list, name, &log) {
      Ok(child) => bridges.push((name.clone(), child)),
      Err(e) => {
        kill_bridges(&mut bridges);
        return Err(e);
      }
    }
  }
  let mut kick_bridges = Vec::new();
  for channel in &kick_channels {
    thread::sleep(STAGGER);
    let slug = &channel.slug;
    let log = paths.logs.join(format!("{}.log", kick_capture_log(slug)));
    let started =
      bridge::start_kick(&build, &paths.data, &kick_list, slug, &log);
    match started {
      Ok(child) => kick_bridges.push((slug.clone(), child)),
      Err(e) => problems.push(format!("{slug}: no Kick bridge: {e}")),
    }
  }
  // Each dgg bridge, waited on together: one that isn't healthy in time
  // is warned of, as a Kick one is, since once any has taken over, what
  // was installed stops everything it runs.
  let until = Instant::now() + options.healthy_within;
  let mut healthy = Vec::new();
  for (name, mut child) in bridges {
    let left = until.saturating_duration_since(Instant::now());
    if wait_healthy(&journal_root, &name, since, left)?.is_some() {
      healthy.push((name, child));
      continue;
    }
    let _ = child.kill();
    let _ = child.wait();
    problems.push(format!(
      "{name}: a capture from the new build wasn't healthy within {}s \
       (see {}.log): it may miss a few minutes",
      options.healthy_within.as_secs(),
      dgg_capture_log(&name)
    ));
  }
  let bridges = healthy;
  // Each Kick bridge, waited on together: one that isn't healthy in time
  // is only warned of, as dgg's captures come first.
  let kick_root = kick_root(&journal_root);
  let until = Instant::now() + options.healthy_within;
  let mut healthy = Vec::new();
  for (slug, mut child) in kick_bridges {
    let left = until.saturating_duration_since(Instant::now());
    if wait_kick_healthy(&kick_root, &slug, since, left)?.is_some() {
      healthy.push((slug, child));
      continue;
    }
    let _ = child.kill();
    let _ = child.wait();
    problems.push(format!(
      "{slug}: a Kick capture from the new build wasn't healthy within {}s \
       (see {}.log): it may miss a few seconds",
      options.healthy_within.as_secs(),
      kick_capture_log(&slug)
    ));
  }
  let kick_bridges = healthy;
  // With no dgg bridge healthy, none has taken over, so what was installed
  // runs on: the install stops here. A Kick bridge that has taken over is
  // left capturing, or its channel would be captured by nothing.
  if bridges.is_empty() && !dgg_channels.is_empty() {
    let mut failed = format!(
      "no dgg capture from the new build was healthy within {}s, so what \
       was installed runs on; see the logs in {}",
      options.healthy_within.as_secs(),
      paths.logs.display()
    );
    if !kick_bridges.is_empty() {
      let slugs: Vec<&str> =
        kick_bridges.iter().map(|(slug, _)| slug.as_str()).collect();
      failed += &format!(
        ". The Kick bridges of {} are left capturing, until the next \
         install takes over",
        slugs.join(", ")
      );
    }
    for problem in &problems {
      failed += &format!("\n  {problem}");
    }
    return Err(error(failed));
  }

  // 2. What was installed stops.
  if let Some(current) = &current {
    let old = current.paths();
    if !moving {
      // Sharing the data, its captures hand off to the bridges.
      // An older build's dgg capture is `capture`.
      let captures: &[&str] = match kick_bridges.is_empty() {
        false => &[
          "dgg-logger dgg-capture",
          "dgg-logger capture",
          "dgg-logger kick-capture",
        ],
        true => &["dgg-logger dgg-capture", "dgg-logger capture"],
      };
      processes::wait_gone_except(
        &old.bin(),
        captures,
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
    moving::rename_logs(&paths.logs, "capture", "dgg-capture")?;
    if !same(&old.root, &paths.root) {
      problems.extend(peer::move_all(&old.root, &paths.root)?);
      if options.scope == Scope::Machine {
        allow_service(&paths)?;
      }
      for list in lists::FILES {
        fs::remove_file(old.root.join(list)).missing_ok()?;
      }
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

  let secure = |file: &Path| match options.scope {
    Scope::Machine => acl::secret(file, &[&service::account(service::NAME)]),
    Scope::User => Ok(()),
  };
  // Nothing installed before reads the plain lists now.
  lists_done.extend(lists::retire_plain(&paths.root)?);
  problems.extend(peer::migrate(&paths.root, &secure)?);
  peer::apply(&paths.root, &options.peers, &secure)?;

  // 4. The new install starts, and takes over from the bridge.
  let since = now_ms() - 1000;
  match options.scope {
    Scope::User => {
      task::register(task::NAME, &launch)?;
      task::run(task::NAME)?;
    }
    Scope::Machine => service::start(service::NAME)?,
  }
  // Each channel's new capture, dgg's then Kick's, waited on together, the
  // supervisor starting each a stagger after the last: one that isn't
  // healthy in time is warned of.
  let spread = STAGGER * (dgg_channels.len() + kick_channels.len()) as u32;
  let until = Instant::now() + options.healthy_within + spread;
  let mut dgg = Vec::new();
  for name in &dgg_channels {
    let left = until.saturating_duration_since(Instant::now());
    let healthy = wait_healthy(&journal_root, name, since, left)?;
    if healthy.is_none() {
      problems.push(format!(
        "{name}: the new install's capture wasn't healthy within {}s; see \
         supervisor.log and {}.log",
        (options.healthy_within + spread).as_secs(),
        dgg_capture_log(name)
      ));
    }
    dgg.push((name.clone(), healthy));
  }
  let mut kick = Vec::new();
  for channel in &kick_channels {
    let slug = &channel.slug;
    let left = until.saturating_duration_since(Instant::now());
    let healthy = wait_kick_healthy(&kick_root, slug, since, left)?;
    if healthy.is_none() {
      problems.push(format!(
        "{slug}: the new install's Kick capture wasn't healthy within {}s; \
         see supervisor.log and {}.log",
        options.healthy_within.as_secs(),
        kick_capture_log(slug)
      ));
    }
    kick.push((slug.clone(), healthy));
  }
  let patience = options.handoff_within;
  problems.extend(hand_off(bridges, &dgg, patience, "", dgg_capture_log));
  problems.extend(hand_off(
    kick_bridges,
    &kick,
    patience,
    "Kick ",
    kick_capture_log,
  ));
  let left = prune(&paths.bin(), options.keep)?;
  let found = peer::gather(&paths.root);
  let peers = found
    .pull
    .iter()
    .map(|named| format!("{} ({})", named.name, named.peer.url))
    .collect();
  Ok(Done {
    scope: options.scope,
    paths,
    build,
    replaced: current,
    dgg,
    kick,
    left,
    peers,
    problems: problems.into_iter().chain(found.problems).collect(),
    lists: lists_done,
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
  secure_peers(paths, &[&account])
}

/// In a machine install, the files holding tokens readable only by
/// administrators, SYSTEM and `readers`.
#[cfg(windows)]
fn secure_peers(paths: &Paths, readers: &[&str]) -> io::Result<()> {
  for file in peer::secrets(&paths.root)? {
    acl::secret(&file, readers)?;
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
    None => {}
  }
  processes::kill_under(&paths.bin(), &["dgg-supervisor"])?;
  // They hold tokens; nothing's left to use them.
  peer::remove_all(&paths.root)?;
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
