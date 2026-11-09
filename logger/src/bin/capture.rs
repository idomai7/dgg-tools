//! Capture dgg chat into the journal.

use {
  ::clap::Parser,
  ::logger::cli::Capture,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory; the journal goes in `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Capture,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
