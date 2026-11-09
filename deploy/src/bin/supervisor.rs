//! `dgg-supervisor`: the supervisor as installed, without a console window
//! (`dgg-logger supervise` is the same, in a console). Everything it has to
//! say goes to the logs.

#![windows_subsystem = "windows"]

use {
  ::clap::Parser,
  ::deploy::supervise::{self, Config, Stop},
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

/// Keep capture and index running against a data directory until stopped
/// or until capture hands off to a newer instance.
#[derive(Parser)]
struct Args {
  #[arg(long)]
  data: PathBuf,
  #[arg(long)]
  logs: PathBuf,
  /// Where the logger's binaries are (default: beside this program).
  #[arg(long)]
  bin_dir: Option<PathBuf>,
  /// Minutes between compactions.
  #[arg(long, default_value_t = 60)]
  compact_every: u64,
}

fn main() -> ExitCode {
  let args = Args::parse();
  let Some(bin_dir) = args
    .bin_dir
    .or_else(|| Some(::std::env::current_exe().ok()?.parent()?.to_owned()))
  else {
    return ExitCode::FAILURE;
  };
  let mut config = Config::new(&bin_dir, &args.data, &args.logs);
  config.compact_every = Duration::from_secs(args.compact_every * 60);
  let stop = Stop::new();
  let on_signal = stop.clone();
  let _ = ::ctrlc::set_handler(move || on_signal.request());
  match supervise::run(&config, &stop) {
    Ok(_) => ExitCode::SUCCESS,
    Err(_) => ExitCode::FAILURE,
  }
}
