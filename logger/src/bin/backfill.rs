//! Fetch messages from rustlesearch.dev or linkers.ooo into the journal's

use {
  ::clap::Parser,
  ::logger::cli::Backfill,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory: reads `<data>/index.sqlite`, writes `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Backfill,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
