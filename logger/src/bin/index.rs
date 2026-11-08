//! Index the journal into SQLite.

use {
  ::clap::Parser,
  ::logger::index::{index, open, rebuild},
  ::std::{path::PathBuf, process::ExitCode},
};

/// Index the journal into SQLite.
#[derive(Parser)]
struct Args {
  /// Data directory: reads `<data>/journal`, writes `<data>/index.sqlite`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  /// Delete the index and rebuild it from the whole journal.
  #[arg(long)]
  rebuild: bool,
}

fn main() -> ExitCode {
  let args = Args::parse();
  let path = args.data.join("index.sqlite");
  let opened = if args.rebuild {
    rebuild(&path)
  } else {
    open(&path)
  };
  let result =
    opened.and_then(|mut db| index(&mut db, &args.data.join("journal")));
  match result {
    Ok(stats) => {
      eprintln!(
        "{} lines, {} new messages, {} unparsed",
        stats.lines, stats.messages, stats.unparsed
      );
      for (file, why) in &stats.skipped {
        eprintln!("skipped {file}: {why}");
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("index failed: {e}");
      ExitCode::FAILURE
    }
  }
}
