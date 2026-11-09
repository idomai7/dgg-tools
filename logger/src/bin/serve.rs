//! Serve this host's journal over HTTP, for other hosts.

use {
  ::clap::Parser,
  ::logger::cli::Serve,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory; serves `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Serve,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
