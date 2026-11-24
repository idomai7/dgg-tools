//! The supervisor: keeps the logger's programs running against one data
//! directory.
//!
//! - `dgg-logger dgg-capture` is restarted after it fails. When it exits cleanly it has
//!   handed off to a newer instance, so the supervisor stops everything and
//!   exits too: starting a newer supervisor is how an upgrade happens.
//! - `dgg-logger migrate` runs once, at start, alongside capture: it brings
//!   the journal and the index up to this build's formats, and does nothing
//!   once they are (for now, there's nothing to migrate). It waits for the locks below, so it runs
//!   once an older supervisor has exited. Only once it has succeeded do
//!   index, compact and sync start; if it fails, they wait for the next
//!   start.
//! - `dgg-logger index --follow` is always restarted. Only one supervisor runs it
//!   at a time (`index.lock` in the data directory), so during a handoff the
//!   new one waits for the old one to exit.
//! - `dgg-logger compact` runs every `compact_every` (the first time a minute after
//!   start), by whichever supervisor holds `compact.lock`. The first time,
//!   it's `compact --early`: what captures from before the upgrade (or
//!   restart) left on days still open is compacted then, not once the day
//!   ends.
//! - Given where its peers are (see [`crate::peer`]), `dgg-logger sync`
//!   pulls from each peer in turn every `sync_every` (the first time two
//!   minutes after start), by whichever supervisor holds `sync.lock`. They
//!   are looked for before each round; one failing doesn't stop the rest.
//! - Given a `serve` section in the install's config (see [`crate::peer`]),
//!   read at start, `dgg-logger serve` is always restarted, its tokens and
//!   key on its stdin. Only one supervisor runs it at a time (`serve.lock`),
//!   so during a handoff the new one waits for the old one's port.
//! - Given the Kick channel list, a `dgg-logger kick-capture` runs for each
//!   channel in it, each on its own, so one failing leaves the others; the
//!   list is read every `kick_check_every` to start the added. Each is
//!   restarted after it fails. A clean exit (a handoff to a newer instance,
//!   or its channel removed) ends one alone. When dgg's capture hands off,
//!   Kick's are given `kick_handoff_wait` to hand off too (an upgrade's new
//!   ones are starting alongside) before they're stopped.
//!
//! Each program's stdout and stderr go, line by line, to `<name>.log` in the
//! logs directory, each Kick channel's capture to its own (see
//! [`kick_capture_log`]); the supervisor's own lines to `supervisor.log`. Restarts
//! wait `first`, doubling up to `max`, back to `first` once a run has lasted
//! `reset_after`.

use {
  crate::{lifeline::Lifeline, log::Log, peer},
  ::common::{config::Secret, log::Level},
  ::logger::kick::channels,
  ::std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, File, TryLockError},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
      Arc, Condvar, Mutex,
      atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
  },
};

/// A program to run: its path and arguments, and its config (JSON, with
/// any secrets), written to its stdin, which is then closed.
#[derive(Clone, Debug, Default)]
pub struct Program {
  pub path: PathBuf,
  pub args: Vec<OsString>,
  pub config: Option<Secret>,
}

impl Program {
  /// `name` (with `.exe` on Windows) in `bin_dir`, with `args`.
  pub fn in_dir(bin_dir: &Path, name: &str, args: &[&Path]) -> Self {
    Self {
      path: bin_dir.join(format!("{name}{}", ::std::env::consts::EXE_SUFFIX)),
      args: args.iter().map(|arg| arg.as_os_str().to_owned()).collect(),
      config: None,
    }
  }
}

#[derive(Clone, Debug)]
pub struct Backoff {
  pub first: Duration,
  pub max: Duration,
  pub reset_after: Duration,
}

impl Default for Backoff {
  fn default() -> Self {
    Self {
      first: Duration::from_secs(5),
      max: Duration::from_secs(300),
      reset_after: Duration::from_secs(600),
    }
  }
}

#[derive(Clone, Debug)]
pub struct Config {
  pub dgg_capture: Program,
  pub migrate: Program,
  pub index: Program,
  pub compact: Program,
  /// `compact`'s first run after start.
  pub compact_early: Program,
  /// `sync`, without its peer: that's added from `peers`.
  pub sync: Program,
  /// `serve`, without where it listens or its config: they're added from
  /// the config in `peers`.
  pub serve: Program,
  /// Where the peers to sync from are, if anywhere: the install's root
  /// (see [`crate::peer`]).
  pub peers: Option<PathBuf>,
  /// `kick-capture`, without its channel list or channel: they're added
  /// from `kick_channels`.
  pub kick_capture: Program,
  /// The Kick channel list, if Kick is captured.
  pub kick_channels: Option<PathBuf>,
  pub kick_check_every: Duration,
  pub kick_handoff_wait: Duration,
  /// Holds `index.lock`, `compact.lock` and `sync.lock`.
  pub lock_dir: PathBuf,
  pub logs: PathBuf,
  pub compact_first: Duration,
  pub compact_every: Duration,
  pub sync_first: Duration,
  pub sync_every: Duration,
  pub backoff: Backoff,
  pub log_size: u64,
  pub log_keep: u32,
}

impl Config {
  /// `dgg-logger`'s commands, from `bin_dir`, run against `data`.
  pub fn new(bin_dir: &Path, data: &Path, logs: &Path) -> Self {
    let command = |args: &[&str]| {
      let mut args: Vec<&Path> = args.iter().map(Path::new).collect();
      args.extend([Path::new("--data"), data]);
      Program::in_dir(bin_dir, "dgg-logger", &args)
    };
    Self {
      dgg_capture: command(&["dgg-capture"]),
      migrate: command(&["migrate"]),
      index: command(&["index", "--follow"]),
      compact: command(&["compact"]),
      compact_early: command(&["compact", "--early"]),
      sync: command(&["sync"]),
      serve: command(&["serve"]),
      peers: None,
      kick_capture: command(&["kick-capture"]),
      kick_channels: None,
      kick_check_every: Duration::from_secs(60),
      kick_handoff_wait: Duration::from_secs(90),
      lock_dir: data.to_owned(),
      logs: logs.to_owned(),
      compact_first: Duration::from_secs(60),
      compact_every: Duration::from_secs(3600),
      sync_first: Duration::from_secs(120),
      sync_every: Duration::from_secs(15 * 60),
      backoff: Backoff::default(),
      log_size: 10 << 20,
      log_keep: 5,
    }
  }
}

/// The Kick channel list of the install whose root is `root`: none without
/// one.
pub fn kick_channels(root: Option<&Path>) -> Option<PathBuf> {
  Some(root?.join(channels::FILE))
}

/// A stop request, shared between threads: [`Stop::request`] wakes every
/// [`Stop::wait`].
#[derive(Clone, Default)]
pub struct Stop(Arc<(Mutex<bool>, Condvar)>);

impl Stop {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn request(&self) {
    let (stopped, changed) = &*self.0;
    *stopped.lock().unwrap() = true;
    changed.notify_all();
  }

  pub fn requested(&self) -> bool {
    *self.0.0.lock().unwrap()
  }

  /// Waits up to `timeout`; whether a stop was requested.
  pub fn wait(&self, timeout: Duration) -> bool {
    let (stopped, changed) = &*self.0;
    let guard = stopped.lock().unwrap();
    let (guard, _) = changed
      .wait_timeout_while(guard, timeout, |stopped| !*stopped)
      .unwrap();
    *guard
  }
}

/// Why [`run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
  /// A stop was requested.
  Stopped,
  /// Capture handed off to a newer instance.
  HandedOff,
}

type SharedLog = Arc<Mutex<Log>>;

/// Logs `line` at `level`.
fn log_line(log: &SharedLog, level: Level, line: &str) {
  // Nowhere left to report a failing log to.
  let _ = log.lock().unwrap().line(&format!("{level} {line}"));
}

/// Runs everything until `stop` is requested or capture hands off.
pub fn run(config: &Config, stop: &Stop) -> io::Result<Exit> {
  let open = |name: &str| -> io::Result<SharedLog> {
    let path = config.logs.join(format!("{name}.log"));
    let log = Log::open(path, config.log_size, config.log_keep)?;
    Ok(Arc::new(Mutex::new(log)))
  };
  let supervisor = open("supervisor")?;
  let logs = [
    open("dgg-capture")?,
    open("migrate")?,
    open("index")?,
    open("compact")?,
  ];
  let sync_log = match &config.peers {
    Some(_) => Some(open("sync")?),
    None => None,
  };
  // What to serve with, read once: a change takes effect at the next start.
  let serving = match config.peers.as_deref().map(peer::read_config) {
    Some(Ok(Some(found))) => found.serve.map(|serve| (serve, found.mesh)),
    Some(Err(e)) => {
      let said = format!("serve: {e}; not served until the next start");
      log_line(&supervisor, Level::Error, &said);
      None
    }
    Some(Ok(None)) | None => None,
  };
  let serve_log = match &serving {
    Some(_) => Some(open("serve")?),
    None => None,
  };
  fs::create_dir_all(&config.lock_dir)?;
  log_line(
    &supervisor,
    Level::Info,
    &format!(
      "supervisor {} started (build {})",
      ::std::process::id(),
      crate::BUILD
    ),
  );

  // Stops the programs: on request, or once capture hands off.
  let programs = Stop::new();
  let lifeline = Lifeline::new()?;
  let context = Context {
    supervisor: &supervisor,
    stop: &programs,
    lifeline: &lifeline,
  };
  let handed_off = AtomicBool::new(false);
  // Kick's capture is stopped apart, to let it hand off on its own.
  let kick = Stop::new();
  let kick_context = Context {
    supervisor: &supervisor,
    stop: &kick,
    lifeline: &lifeline,
  };
  // How many are running, to wait only for those.
  let kick_running = AtomicUsize::new(0);
  // References, so the threads started from the migration's can take them.
  let (context, programs, sync_log, serve_log, supervisor) =
    (&context, &programs, &sync_log, &serve_log, &supervisor);
  thread::scope(|scope| {
    if let Some(list) = &config.kick_channels {
      scope.spawn(|| {
        capture_kick(config, list, &kick_running, &kick_context);
      });
    }
    let [capture_log, migrate_log, index_log, compact_log] = &logs;
    scope.spawn(|| {
      let ended = keep_running(
        &config.dgg_capture,
        "dgg-capture",
        capture_log,
        &config.backoff,
        true,
        context,
      );
      if ended {
        log_line(supervisor, Level::Info, "capture handed off; stopping");
        handed_off.store(true, Ordering::SeqCst);
      }
      programs.request();
    });
    // The rest wait for the migration.
    scope.spawn(move || {
      match run_once(&config.migrate, "migrate", None, migrate_log, context) {
        Some(Some(status)) if status.success() => {}
        // Stopped.
        None => return,
        Some(_) => {
          log_line(
            supervisor,
            Level::Error,
            "migrate failed; index, compact, sync and serve wait for the next \
             start",
          );
          return;
        }
      }
      if let (Some((serve, mesh)), Some(serve_log)) = (serving, serve_log) {
        scope.spawn(move || {
          let Some(stdin) = serve.config(mesh.as_ref()) else {
            log_line(
              supervisor,
              Level::Error,
              "serve has no read token, and there's no mesh to take one \
               from; not served",
            );
            return;
          };
          let lock = config.lock_dir.join("serve.lock");
          let Some(_lock) = hold(&lock, supervisor, programs) else {
            return;
          };
          let mut program = config.serve.clone();
          program.args.extend([
            "--listen".into(),
            serve.listen.into(),
            "--config-stdin".into(),
          ]);
          program.config = Some(stdin);
          keep_running(
            &program,
            "serve",
            serve_log,
            &config.backoff,
            false,
            context,
          );
        });
      }
      scope.spawn(move || {
        let lock = config.lock_dir.join("index.lock");
        let Some(_lock) = hold(&lock, supervisor, programs) else {
          return;
        };
        keep_running(
          &config.index,
          "index",
          index_log,
          &config.backoff,
          false,
          context,
        );
      });
      scope.spawn(move || {
        let schedule = Schedule {
          name: "compact",
          lock: config.lock_dir.join("compact.lock"),
          first: config.compact_first,
          every: config.compact_every,
        };
        let first = AtomicBool::new(true);
        periodically(&schedule, compact_log, context, || {
          let program = match first.swap(false, Ordering::SeqCst) {
            true => &config.compact_early,
            false => &config.compact,
          };
          vec![(None, program.clone())]
        });
      });
      let (Some(peers), Some(sync_log)) = (&config.peers, sync_log) else {
        return;
      };
      scope.spawn(move || {
        let schedule = Schedule {
          name: "sync",
          lock: config.lock_dir.join("sync.lock"),
          first: config.sync_first,
          every: config.sync_every,
        };
        // What the peers were last round, to report only changes.
        let last = Mutex::new(None::<String>);
        // The last that could be read, kept while the config can't be.
        let good = Mutex::new(None::<peer::Peers>);
        periodically(&schedule, sync_log, context, || {
          let mut found = peer::gather(peers);
          let mut good = good.lock().unwrap();
          match (found.unreadable, &*good) {
            (true, Some(good)) => {
              found.pull.clone_from(&good.pull);
              found.ca.clone_from(&good.ca);
              found.problems.push("syncing as before".into());
            }
            (true, None) => {}
            (false, _) => *good = Some(found.clone()),
          }
          let mut said = if found.pull.is_empty() {
            format!("sync: no peers in {}", peers.display())
          } else {
            let names: Vec<String> = found
              .pull
              .iter()
              .map(|named| format!("{} ({})", named.name, named.peer.url))
              .collect();
            format!("sync: from {}", names.join(", "))
          };
          for problem in &found.problems {
            said += &format!("; {problem}");
          }
          let level = match found.problems.is_empty() {
            true => Level::Info,
            false => Level::Warn,
          };
          let mut last = last.lock().unwrap();
          if last.as_ref() != Some(&said) {
            log_line(supervisor, level, &said);
            *last = Some(said);
          }
          found
            .pull
            .iter()
            .map(|named| {
              (
                Some(named.name.clone()),
                sync_from(&config.sync, &named.peer, found.ca.as_deref()),
              )
            })
            .collect()
        });
      });
    });
    while !programs.wait(Duration::from_millis(200)) {
      if stop.requested() {
        log_line(supervisor, Level::Info, "stopping");
        programs.request();
      }
    }
    if handed_off.load(Ordering::SeqCst)
      && kick_running.load(Ordering::SeqCst) > 0
    {
      let wait = config.kick_handoff_wait;
      log_line(
        supervisor,
        Level::Info,
        &format!(
          "waiting up to {}s for kick-capture to hand off",
          wait.as_secs_f64()
        ),
      );
      let until = Instant::now() + wait;
      while kick_running.load(Ordering::SeqCst) > 0
        && Instant::now() < until
        && !stop.requested()
      {
        thread::sleep(Duration::from_millis(200));
      }
    }
    kick.request();
  });
  let exit = if handed_off.load(Ordering::SeqCst) {
    Exit::HandedOff
  } else {
    Exit::Stopped
  };
  log_line(
    supervisor,
    Level::Info,
    &format!("supervisor exiting: {exit:?}"),
  );
  Ok(exit)
}

/// The name of the log Kick channel `slug`'s capture writes to, bridges
/// too: `kick-capture.<slug>`, as slugs hold no dots.
pub fn kick_capture_log(slug: &str) -> String {
  format!("kick-capture.{slug}")
}

/// Runs a `kick-capture` for each channel in `list`, each on its own, and
/// restarted after it fails, reading the list every `kick_check_every` to
/// start the added; until stopped, with `running` counting those running.
/// A clean exit (a handoff, or its channel removed) ends one alone: it's
/// started again only once its channel has left the list and come back.
fn capture_kick(
  config: &Config,
  list: &Path,
  running: &AtomicUsize,
  context: &Context,
) {
  // What was last said of the list, to say only changes.
  let mut said = None;
  // The channels with a capture, running or ended.
  let mut started: HashSet<String> = HashSet::new();
  // Those whose capture ended cleanly.
  let ended: Mutex<HashSet<String>> = Mutex::default();
  thread::scope(|scope| {
    loop {
      let now = match channels::load(list) {
        Ok(found) => {
          let listed: HashSet<String> =
            found.into_iter().map(|channel| channel.slug).collect();
          ended.lock().unwrap().retain(|slug| {
            let still = listed.contains(slug);
            if !still {
              started.remove(slug);
            }
            still
          });
          for slug in &listed {
            if !started.insert(slug.clone()) {
              continue;
            }
            let path =
              config.logs.join(format!("{}.log", kick_capture_log(slug)));
            let log = match Log::open(path, config.log_size, config.log_keep) {
              Ok(log) => Arc::new(Mutex::new(log)),
              Err(e) => {
                let said = format!("kick-capture {slug}: no log: {e}");
                log_line(context.supervisor, Level::Error, &said);
                // Tried again at the next look.
                started.remove(slug);
                continue;
              }
            };
            let program = kick_capture(config, list, slug);
            let (ended, slug) = (&ended, slug.clone());
            scope.spawn(move || {
              let name = format!("kick-capture {slug}");
              running.fetch_add(1, Ordering::SeqCst);
              let clean = keep_running(
                &program,
                &name,
                &log,
                &config.backoff,
                true,
                context,
              );
              running.fetch_sub(1, Ordering::SeqCst);
              if clean {
                let said = format!("{name}: ended (handed off, or removed)");
                log_line(context.supervisor, Level::Info, &said);
                ended.lock().unwrap().insert(slug);
              }
            });
          }
          match listed.is_empty() {
            true => Some((
              Level::Info,
              format!("kick-capture: no channels in {}", list.display()),
            )),
            false => None,
          }
        }
        Err(e) => Some((Level::Warn, format!("kick-capture: {e}"))),
      };
      if said != now {
        if let Some((level, line)) = &now {
          log_line(context.supervisor, *level, line);
        }
        said = now;
      }
      if context.stop.wait(config.kick_check_every) {
        break;
      }
    }
  });
}

/// `kick-capture` for channel `slug` in `list`.
fn kick_capture(config: &Config, list: &Path, slug: &str) -> Program {
  let mut program = config.kick_capture.clone();
  program.args.extend([
    "--channels".into(),
    list.as_os_str().to_owned(),
    "--channel".into(),
    slug.into(),
  ]);
  program
}

/// `sync` pulling from `peer`, its token in its config on stdin; if it's
/// `https://`, trusting the CA certificate `ca` (PEM) alone, if given.
fn sync_from(sync: &Program, peer: &peer::Peer, ca: Option<&str>) -> Program {
  let mut program = sync.clone();
  program.args.extend([
    "--peer".into(),
    peer.url.clone().into(),
    "--config-stdin".into(),
  ]);
  let mut config = ::serde_json::json!({ "token": peer.token.expose() });
  if let Some(ca) = ca.filter(|_| peer.url.starts_with("https://")) {
    config["ca"] = ca.into();
  }
  program.config = Some(Secret::new(config.to_string()));
  program
}

/// When to run a program over and over, and the lock that keeps it to one
/// supervisor at a time.
struct Schedule {
  name: &'static str,
  lock: PathBuf,
  first: Duration,
  every: Duration,
}

/// Runs the programs `programs` gives on `schedule`, one after another,
/// until stopped. Each one's lines in `log` start with its label, if it has
/// one.
fn periodically(
  schedule: &Schedule,
  log: &SharedLog,
  context: &Context,
  programs: impl Fn() -> Vec<(Option<String>, Program)>,
) {
  let Schedule {
    name,
    lock,
    first,
    every,
  } = schedule;
  let mut wait = *first;
  while !context.stop.wait(wait) {
    wait = *every;
    match try_hold(lock) {
      Ok(Some(_lock)) => {
        for (label, program) in programs() {
          let named = match &label {
            Some(label) => format!("{name} {label}"),
            None => (*name).to_owned(),
          };
          if run_once(&program, &named, label.as_deref(), log, context)
            .is_none()
          {
            break;
          }
        }
      }
      Ok(None) => log_line(
        context.supervisor,
        Level::Warn,
        &format!("{name}: another supervisor has it"),
      ),
      Err(e) => {
        log_line(context.supervisor, Level::Warn, &format!("{name}: {e}"))
      }
    }
  }
}

/// Takes `path`'s lock if it's free.
fn try_hold(path: &Path) -> io::Result<Option<File>> {
  let file = File::create(path)?;
  match file.try_lock() {
    Ok(()) => Ok(Some(file)),
    Err(TryLockError::WouldBlock) => Ok(None),
    Err(TryLockError::Error(e)) => Err(e),
  }
}

/// Waits for `path`'s lock; `None` if stopped first.
fn hold(path: &Path, supervisor: &SharedLog, stop: &Stop) -> Option<File> {
  let mut said = false;
  loop {
    match try_hold(path) {
      Ok(Some(file)) => return Some(file),
      Ok(None) if !said => {
        log_line(
          supervisor,
          Level::Warn,
          &format!("waiting for {}: another supervisor has it", path.display()),
        );
        said = true;
      }
      Ok(None) => {}
      Err(e) => {
        log_line(supervisor, Level::Warn, &format!("{}: {e}", path.display()))
      }
    }
    if stop.wait(Duration::from_secs(1)) {
      return None;
    }
  }
}

/// What every program run shares: where the supervisor's own lines go, the
/// stop request, and the lifeline the programs are tied to.
struct Context<'a> {
  supervisor: &'a SharedLog,
  stop: &'a Stop,
  lifeline: &'a Lifeline,
}

/// Runs `program` again and again, waiting longer after each quick failure,
/// until stopped. With `until_clean`, a clean exit ends it too: then `true`.
fn keep_running(
  program: &Program,
  name: &str,
  log: &SharedLog,
  backoff: &Backoff,
  until_clean: bool,
  context: &Context,
) -> bool {
  let Context {
    supervisor, stop, ..
  } = context;
  let mut delay = backoff.first;
  loop {
    let started = Instant::now();
    let Some(status) = run_once(program, name, None, log, context) else {
      return false;
    };
    if until_clean && status.is_some_and(|status| status.success()) {
      return true;
    }
    if started.elapsed() >= backoff.reset_after {
      delay = backoff.first;
    }
    log_line(
      supervisor,
      Level::Warn,
      &format!("{name}: restarting in {}s", delay.as_secs_f64()),
    );
    if stop.wait(delay) {
      return false;
    }
    delay = (delay * 2).min(backoff.max);
  }
}

/// Runs `program` once, its output to `log`, each line after `label` if
/// given. `None` if stopped (the program is killed), else `Some` of its
/// exit status, or `Some(None)` if it couldn't be started.
fn run_once(
  program: &Program,
  name: &str,
  label: Option<&str>,
  log: &SharedLog,
  context: &Context,
) -> Option<Option<ExitStatus>> {
  let Context {
    supervisor,
    stop,
    lifeline,
  } = context;
  let mut command = Command::new(&program.path);
  command
    .args(&program.args)
    .stdin(match program.config {
      Some(_) => Stdio::piped(),
      None => Stdio::null(),
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  #[cfg(windows)]
  {
    use ::std::os::windows::process::CommandExt;
    // No console window, and no share of the supervisor's Ctrl-C.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
  }
  let mut child = match command.spawn() {
    Ok(child) => child,
    Err(e) => {
      log_line(
        supervisor,
        Level::Error,
        &format!("{name}: can't start {}: {e}", program.path.display()),
      );
      return Some(None);
    }
  };
  log_line(
    supervisor,
    Level::Info,
    &format!("{name}: started, pid {}", child.id()),
  );
  if let Err(e) = lifeline.tie(&child) {
    // It runs anyway; it just might outlive a killed supervisor.
    log_line(supervisor, Level::Warn, &format!("{name}: {e}"));
  }
  if let (Some(config), Some(mut stdin)) =
    (program.config.clone(), child.stdin.take())
  {
    // Apart, so a program that doesn't read it can't hold this up; closed
    // once written.
    let (supervisor, name) = (Arc::clone(supervisor), name.to_owned());
    thread::spawn(move || {
      if let Err(e) = stdin.write_all(config.expose().as_bytes()) {
        let said = format!("{name}: can't pass on its config: {e}");
        log_line(&supervisor, Level::Warn, &said);
      }
    });
  }
  let readers: Vec<_> = [
    child
      .stdout
      .take()
      .map(|out| Box::new(out) as Box<dyn Read + Send>),
    child
      .stderr
      .take()
      .map(|err| Box::new(err) as Box<dyn Read + Send>),
  ]
  .into_iter()
  .flatten()
  .map(|output| {
    let log = log.clone();
    let prefix = label.map(|label| format!("{label}: ")).unwrap_or_default();
    thread::spawn(move || copy_lines(output, &prefix, &log))
  })
  .collect();
  match wait_or_kill(&mut child, stop) {
    Some(status) => {
      // Its last lines. (After a kill they're not waited for: anything the
      // program started could still hold its output open.)
      for reader in readers {
        let _ = reader.join();
      }
      let level = match status.success() {
        true => Level::Info,
        false => Level::Error,
      };
      log_line(supervisor, level, &format!("{name}: exited, {status}"));
      Some(Some(status))
    }
    None => {
      log_line(supervisor, Level::Info, &format!("{name}: stopped"));
      None
    }
  }
}

/// Waits for `child` to exit; `None` if `stop` came first, after killing
/// it.
fn wait_or_kill(child: &mut Child, stop: &Stop) -> Option<ExitStatus> {
  loop {
    match child.try_wait() {
      Ok(Some(status)) => return Some(status),
      Ok(None) => {}
      Err(_) => return child.wait().ok(),
    }
    if stop.wait(Duration::from_millis(200)) {
      let _ = child.kill();
      let _ = child.wait();
      return None;
    }
  }
}

fn copy_lines(output: Box<dyn Read + Send>, prefix: &str, log: &SharedLog) {
  let mut reader = BufReader::new(output);
  let mut line = Vec::new();
  loop {
    line.clear();
    match reader.read_until(b'\n', &mut line) {
      Ok(0) | Err(_) => return,
      Ok(_) => {
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        // A line from a program that doesn't say its level is information.
        let (level, text) = Level::split(text).unwrap_or((Level::Info, text));
        log_line(log, level, &format!("{prefix}{text}"));
      }
    }
  }
}
