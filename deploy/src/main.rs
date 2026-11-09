//! Run, install and inspect the logger as a long-lived service.

use {
  ::clap::{Parser, Subcommand},
  ::deploy::supervise::{self, Config, Exit, Stop},
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
}

fn main() -> ExitCode {
  match Args::parse().command {
    Command::Supervise {
      data,
      logs,
      bin_dir,
      compact_every,
    } => {
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
  }
}

fn beside_this_program() -> Option<PathBuf> {
  Some(::std::env::current_exe().ok()?.parent()?.to_owned())
}
