//! Index the journal into SQLite.

use {
  ::clap::Parser,
  ::logger::cli::Index,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory: reads `<data>/journal`, writes `<data>/index.sqlite`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Index,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
