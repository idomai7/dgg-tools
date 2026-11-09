//! Compact closed days of the journal into zstd archives.

use {
  ::clap::Parser,
  ::logger::cli::Compact,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory; compacts `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Compact,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
