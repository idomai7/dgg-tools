//! Pull another host's journal into this one's.

use {
  ::clap::Parser,
  ::logger::cli::Sync,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory; writes `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Sync,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
