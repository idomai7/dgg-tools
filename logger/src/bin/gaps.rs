//! Report gaps in what was captured.

use {
  ::clap::Parser,
  ::logger::cli::Gaps,
  ::std::{path::PathBuf, process::ExitCode},
};

#[derive(Parser)]
struct Args {
  /// Data directory: reads `<data>/journal` and `<data>/index.sqlite`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  #[command(flatten)]
  command: Gaps,
}

fn main() -> ExitCode {
  let args = Args::parse();
  args.command.run(&args.data)
}
