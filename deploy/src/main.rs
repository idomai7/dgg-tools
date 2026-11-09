//! Run, install and inspect the logger as a long-lived service.

use {
  ::clap::{Parser, Subcommand},
  ::deploy::{
    install,
    paths::{Paths, user_root},
    status::{self, LOGS, Watcher},
    supervise::{self, Config, Exit, Stop},
  },
  ::logger::cli,
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

#[derive(Parser)]
struct Args {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Keep capture and `index --follow` running against a data directory,
  /// compacting every hour, until stopped (Ctrl-C) or until capture hands
  /// off to a newer instance.
  Supervise {
    /// Data directory: the journal and the index.
    #[arg(long)]
    data: PathBuf,
    /// Where the logs go.
    #[arg(long)]
    logs: PathBuf,
    /// Where `dgg-logger` is, to run its commands (default:
    /// beside this program).
    #[arg(long)]
    bin_dir: Option<PathBuf>,
    /// Minutes between compactions.
    #[arg(long, default_value_t = 60)]
    compact_every: u64,
  },
  /// Install the logger from the build this program is in: for this user,
  /// as a scheduled task started at logon, or with `--machine` as a
  /// Windows service started with the system. Installing again upgrades;
  /// installing the other kind switches to it, moving the data. Takes over
  /// an install made by the PowerShell scripts. Nothing is missed on the
  /// way: a capture from the new build bridges the change.
  #[cfg(windows)]
  Install {
    /// A machine install (a service): needs an elevated prompt.
    #[arg(long)]
    machine: bool,
    /// Where the install goes (default: the installed one's, else
    /// `%LOCALAPPDATA%\dgg-tools`, or `C:\ProgramData\dgg-tools` with
    /// `--machine`).
    #[arg(long)]
    root: Option<PathBuf>,
    /// The data directory, if not `<root>\data`.
    #[arg(long)]
    data: Option<PathBuf>,
    /// The logs directory, if not `<root>\logs`.
    #[arg(long)]
    logs: Option<PathBuf>,
    /// Builds to keep in `<root>\bin`.
    #[arg(long, default_value_t = 3)]
    keep: usize,
  },
  /// Remove the install: its task or service, programs and binaries. The
  /// data and logs stay (a machine install's get ordinary permissions,
  /// owned by you). A machine install needs an elevated prompt.
  #[cfg(windows)]
  Uninstall {
    /// Where to clean up if nothing is installed (after shell runs).
    #[arg(long)]
    root: Option<PathBuf>,
  },
  /// Show what the logger is doing: its programs, each capture's health,
  /// the journal, the index, free space and the ends of the logs.
  Status {
    /// The install's root, holding `bin`, `data` and `logs` (default: the
    /// installed one, else the user install's).
    #[arg(long)]
    root: Option<PathBuf>,
    /// The data directory, if not `<root>/data`.
    #[arg(long)]
    data: Option<PathBuf>,
    /// The logs directory, if not `<root>/logs`.
    #[arg(long)]
    logs: Option<PathBuf>,
    /// Stay open, redrawing every `--interval` seconds, with a log below.
    #[arg(long)]
    watch: bool,
    #[arg(long, default_value_t = 2.0)]
    interval: f64,
    /// Lines of each log to show, without `--watch`.
    #[arg(long, default_value_t = 5)]
    lines: usize,
  },
  Capture(WithData<cli::Capture>),
  Index(WithData<cli::Index>),
  Compact(WithData<cli::Compact>),
  Serve(WithData<cli::Serve>),
  Sync(WithData<cli::Sync>),
  Backfill(WithData<cli::Backfill>),
  Gaps(WithData<cli::Gaps>),
}

// One of the logger's own commands, with the data directory it works on
// (not a doc comment, which would replace the command's own).
#[derive(::clap::Args)]
struct WithData<T: ::clap::Args> {
  /// Data directory, holding the journal and the index (default: the
  /// installed logger's).
  #[arg(long)]
  data: Option<PathBuf>,
  #[command(flatten)]
  command: T,
}

impl<T: ::clap::Args> WithData<T> {
  /// `run` on the data directory.
  fn run(
    self,
    run: impl FnOnce(T, &::std::path::Path) -> ExitCode,
  ) -> ExitCode {
    let data = self.data.unwrap_or_else(|| installed().data);
    run(self.command, &data)
  }
}

fn main() -> ExitCode {
  match Args::parse().command {
    Command::Supervise {
      data,
      logs,
      bin_dir,
      compact_every,
    } => supervise(data, logs, bin_dir, compact_every),
    #[cfg(windows)]
    Command::Install {
      machine,
      root,
      data,
      logs,
      keep,
    } => install(&install::Options {
      scope: if machine {
        install::Scope::Machine
      } else {
        install::Scope::User
      },
      root,
      data,
      logs,
      keep: keep.max(1),
      ..install::Options::default()
    }),
    #[cfg(windows)]
    Command::Uninstall { root } => uninstall(root),
    Command::Status {
      root,
      data,
      logs,
      watch,
      interval,
      lines,
    } => {
      let mut paths = match root {
        Some(root) => Paths::under(root),
        None => installed(),
      };
      paths.data = data.unwrap_or(paths.data);
      paths.logs = logs.unwrap_or(paths.logs);
      let result = if watch {
        status::watch::run(&paths, Duration::from_secs_f64(interval))
      } else {
        print_status(&paths, lines)
      };
      match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
          eprintln!("status failed: {e}");
          ExitCode::FAILURE
        }
      }
    }
    Command::Capture(command) => command.run(cli::Capture::run),
    Command::Index(command) => command.run(cli::Index::run),
    Command::Compact(command) => command.run(cli::Compact::run),
    Command::Serve(command) => command.run(cli::Serve::run),
    Command::Sync(command) => command.run(cli::Sync::run),
    Command::Backfill(command) => command.run(cli::Backfill::run),
    Command::Gaps(command) => command.run(cli::Gaps::run),
  }
}

/// The installed logger's directories, else a user install's.
fn installed() -> Paths {
  match install::installed() {
    Ok(Some(installed)) => installed.paths(),
    _ => Paths::under(user_root()),
  }
}

#[cfg(windows)]
fn install(options: &install::Options) -> ExitCode {
  eprintln!("installing; waiting for the new capture to be healthy");
  match install::install(options) {
    Ok(done) => {
      let kind = match done.scope {
        install::Scope::User => "user install (a logon task)",
        install::Scope::Machine => "machine install (a service)",
      };
      match &done.replaced {
        None => eprintln!("installed: {kind}"),
        Some(install::Installed::Legacy(_)) => {
          eprintln!("took over from the PowerShell install: {kind}")
        }
        Some(replaced) if replaced.scope() == done.scope => {
          eprintln!("upgraded: {kind}")
        }
        Some(_) => eprintln!("switched to a {kind}"),
      }
      eprintln!("  build    {}", done.build.display());
      eprintln!("  data     {}", done.paths.data.display());
      eprintln!("  logs     {}", done.paths.logs.display());
      eprintln!("  capture  {} (healthy)", done.capture);
      eprintln!("`dgg-logger status --watch` to see it");
      for build in done.left {
        eprintln!("still in use, left for next time: {}", build.display());
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("install failed: {e}");
      ExitCode::FAILURE
    }
  }
}

#[cfg(windows)]
fn uninstall(root: Option<PathBuf>) -> ExitCode {
  match install::uninstall(root) {
    Ok(removed) => {
      match removed.paths {
        Some(paths) => eprintln!(
          "removed the task and the binaries in {}",
          paths.bin().display()
        ),
        None => eprintln!("nothing installed"),
      }
      for path in removed.kept {
        eprintln!("kept {}", path.display());
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("uninstall failed: {e}");
      ExitCode::FAILURE
    }
  }
}

fn print_status(paths: &Paths, lines: usize) -> ::std::io::Result<()> {
  let report = Watcher::new()?.gather(paths)?;
  for line in status::text(&report) {
    println!("{line}");
  }
  for log in LOGS {
    let path = paths.logs.join(format!("{log}.log"));
    if let Ok(tail) = status::tail(&path, lines) {
      println!();
      println!("LOG {}", path.display());
      for line in tail {
        println!("  {line}");
      }
    }
  }
  Ok(())
}

fn supervise(
  data: PathBuf,
  logs: PathBuf,
  bin_dir: Option<PathBuf>,
  compact_every: u64,
) -> ExitCode {
  let bin_dir = match bin_dir {
    Some(bin_dir) => bin_dir,
    None => match beside_this_program() {
      Some(bin_dir) => bin_dir,
      None => {
        eprintln!("can't tell where this program is; pass --bin-dir");
        return ExitCode::FAILURE;
      }
    },
  };
  let mut config = Config::new(&bin_dir, &data, &logs);
  config.compact_every = Duration::from_secs(compact_every * 60);
  let stop = Stop::new();
  let on_signal = stop.clone();
  if let Err(e) = ::ctrlc::set_handler(move || on_signal.request()) {
    eprintln!("can't handle Ctrl-C: {e}");
    return ExitCode::FAILURE;
  }
  eprintln!("supervising; logs in {}", logs.display());
  match supervise::run(&config, &stop) {
    Ok(Exit::Stopped) => ExitCode::SUCCESS,
    Ok(Exit::HandedOff) => {
      eprintln!("capture handed off to a newer instance");
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("supervise failed: {e}");
      ExitCode::FAILURE
    }
  }
}

fn beside_this_program() -> Option<PathBuf> {
  Some(::std::env::current_exe().ok()?.parent()?.to_owned())
}
