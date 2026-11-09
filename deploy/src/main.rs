//! Run, install and inspect the logger as a long-lived service.

use {
  ::clap::{Parser, Subcommand},
  ::deploy::{
    paths::{Paths, user_root},
    status::{self, LOGS, Watcher},
    supervise::{self, Config, Exit, Stop},
  },
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
    /// Where `dgg-capture`, `dgg-index` and `dgg-compact` are (default:
    /// beside this program).
    #[arg(long)]
    bin_dir: Option<PathBuf>,
    /// Minutes between compactions.
    #[arg(long, default_value_t = 60)]
    compact_every: u64,
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
}

fn main() -> ExitCode {
  match Args::parse().command {
    Command::Supervise {
      data,
      logs,
      bin_dir,
      compact_every,
    } => supervise(data, logs, bin_dir, compact_every),
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
  }
}

/// The installed logger's directories: the PowerShell-era install's if
/// there is one, else the user install's.
fn installed() -> Paths {
  #[cfg(windows)]
  if let Ok(Some(legacy)) = ::deploy::legacy::find(::deploy::legacy::FOLDER) {
    return Paths {
      root: legacy.root,
      data: legacy.data,
      logs: legacy.logs,
    };
  }
  Paths::under(user_root())
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
