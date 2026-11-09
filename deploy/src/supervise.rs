//! The supervisor: keeps the logger's programs running against one data
//! directory.
//!
//! - `dgg-capture` is restarted after it fails. When it exits cleanly it has
//!   handed off to a newer instance, so the supervisor stops everything and
//!   exits too: starting a newer supervisor is how an upgrade happens.
//! - `dgg-index --follow` is always restarted. Only one supervisor runs it
//!   at a time (`index.lock` in the data directory), so during a handoff the
//!   new one waits for the old one to exit.
//! - `dgg-compact` runs every `compact_every` (the first time a minute after
//!   start), by whichever supervisor holds `compact.lock`.
//!
//! Each program's stdout and stderr go, line by line, to `<name>.log` in the
//! logs directory; the supervisor's own lines to `supervisor.log`. Restarts
//! wait `first`, doubling up to `max`, back to `first` once a run has lasted
//! `reset_after`.

use {
  crate::{lifeline::Lifeline, log::Log},
  ::std::{
    ffi::OsString,
    fs::{self, File, TryLockError},
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
      Arc, Condvar, Mutex,
      atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
  },
};

/// A program to run: its path and arguments.
#[derive(Clone, Debug)]
pub struct Program {
  pub path: PathBuf,
  pub args: Vec<OsString>,
}

impl Program {
  /// `name` (with `.exe` on Windows) in `bin_dir`, with `args`.
  pub fn in_dir(bin_dir: &Path, name: &str, args: &[&Path]) -> Self {
    Self {
      path: bin_dir.join(format!("{name}{}", ::std::env::consts::EXE_SUFFIX)),
      args: args.iter().map(|arg| arg.as_os_str().to_owned()).collect(),
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
  pub capture: Program,
  pub index: Program,
  pub compact: Program,
  /// Holds `index.lock` and `compact.lock`.
  pub lock_dir: PathBuf,
  pub logs: PathBuf,
  pub compact_first: Duration,
  pub compact_every: Duration,
  pub backoff: Backoff,
  pub log_size: u64,
  pub log_keep: u32,
}

impl Config {
  /// The logger's programs in `bin_dir`, run against `data`.
  pub fn new(bin_dir: &Path, data: &Path, logs: &Path) -> Self {
    let follow = Path::new("--follow");
    let data_flag = Path::new("--data");
    Self {
      capture: Program::in_dir(bin_dir, "dgg-capture", &[data_flag, data]),
      index: Program::in_dir(bin_dir, "dgg-index", &[data_flag, data, follow]),
      compact: Program::in_dir(bin_dir, "dgg-compact", &[data_flag, data]),
      lock_dir: data.to_owned(),
      logs: logs.to_owned(),
      compact_first: Duration::from_secs(60),
      compact_every: Duration::from_secs(3600),
      backoff: Backoff::default(),
      log_size: 10 << 20,
      log_keep: 5,
    }
  }
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

fn log_line(log: &SharedLog, line: &str) {
  // Nowhere left to report a failing log to.
  let _ = log.lock().unwrap().line(line);
}

/// Runs everything until `stop` is requested or capture hands off.
pub fn run(config: &Config, stop: &Stop) -> io::Result<Exit> {
  let open = |name: &str| -> io::Result<SharedLog> {
    let path = config.logs.join(format!("{name}.log"));
    let log = Log::open(path, config.log_size, config.log_keep)?;
    Ok(Arc::new(Mutex::new(log)))
  };
  let supervisor = open("supervisor")?;
  let logs = [open("capture")?, open("index")?, open("compact")?];
  fs::create_dir_all(&config.lock_dir)?;
  log_line(
    &supervisor,
    &format!("supervisor {} started", ::std::process::id()),
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
  thread::scope(|scope| {
    let [capture_log, index_log, compact_log] = &logs;
    scope.spawn(|| {
      let ended = keep_running(
        &config.capture,
        "capture",
        capture_log,
        &config.backoff,
        true,
        &context,
      );
      if ended {
        log_line(&supervisor, "capture handed off; stopping");
        handed_off.store(true, Ordering::SeqCst);
      }
      programs.request();
    });
    scope.spawn(|| {
      let lock = config.lock_dir.join("index.lock");
      let Some(_lock) = hold(&lock, &supervisor, &programs) else {
        return;
      };
      keep_running(
        &config.index,
        "index",
        index_log,
        &config.backoff,
        false,
        &context,
      );
    });
    scope.spawn(|| {
      let lock = config.lock_dir.join("compact.lock");
      let mut wait = config.compact_first;
      while !programs.wait(wait) {
        wait = config.compact_every;
        match try_hold(&lock) {
          Ok(Some(_lock)) => {
            run_once(&config.compact, "compact", compact_log, &context);
          }
          Ok(None) => {
            log_line(&supervisor, "compact: another supervisor has it");
          }
          Err(e) => log_line(&supervisor, &format!("compact: {e}")),
        }
      }
    });
    while !programs.wait(Duration::from_millis(200)) {
      if stop.requested() {
        log_line(&supervisor, "stopping");
        programs.request();
      }
    }
  });
  let exit = if handed_off.load(Ordering::SeqCst) {
    Exit::HandedOff
  } else {
    Exit::Stopped
  };
  log_line(&supervisor, &format!("supervisor exiting: {exit:?}"));
  Ok(exit)
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
          &format!("waiting for {}: another supervisor has it", path.display()),
        );
        said = true;
      }
      Ok(None) => {}
      Err(e) => log_line(supervisor, &format!("{}: {e}", path.display())),
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
    let Some(status) = run_once(program, name, log, context) else {
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
      &format!("{name}: restarting in {}s", delay.as_secs_f64()),
    );
    if stop.wait(delay) {
      return false;
    }
    delay = (delay * 2).min(backoff.max);
  }
}

/// Runs `program` once, its output to `log`. `None` if stopped (the program
/// is killed), else `Some` of its exit status, or `Some(None)` if it
/// couldn't be started.
fn run_once(
  program: &Program,
  name: &str,
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
    .stdin(Stdio::null())
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
        &format!("{name}: can't start {}: {e}", program.path.display()),
      );
      return Some(None);
    }
  };
  log_line(supervisor, &format!("{name}: started, pid {}", child.id()));
  if let Err(e) = lifeline.tie(&child) {
    // It runs anyway; it just might outlive a killed supervisor.
    log_line(supervisor, &format!("{name}: {e}"));
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
    thread::spawn(move || copy_lines(output, &log))
  })
  .collect();
  match wait_or_kill(&mut child, stop) {
    Some(status) => {
      // Its last lines. (After a kill they're not waited for: anything the
      // program started could still hold its output open.)
      for reader in readers {
        let _ = reader.join();
      }
      log_line(supervisor, &format!("{name}: exited, {status}"));
      Some(Some(status))
    }
    None => {
      log_line(supervisor, &format!("{name}: stopped"));
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

fn copy_lines(output: Box<dyn Read + Send>, log: &SharedLog) {
  let mut reader = BufReader::new(output);
  let mut line = Vec::new();
  loop {
    line.clear();
    match reader.read_until(b'\n', &mut line) {
      Ok(0) | Err(_) => return,
      Ok(_) => {
        let text = String::from_utf8_lossy(&line);
        log_line(log, text.trim_end_matches(['\r', '\n']));
      }
    }
  }
}
