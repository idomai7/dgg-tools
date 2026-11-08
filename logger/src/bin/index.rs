//! Index the journal into SQLite.

use {
  ::clap::Parser,
  ::logger::index::{Stats, follow, index, open, rebuild},
  ::std::{
    collections::HashSet, path::PathBuf, process::ExitCode, time::Duration,
  },
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
  /// Keep running, indexing new journal lines as they're written. Stopping
  /// it at any point is safe: an interrupted batch is rolled back and redone.
  #[arg(long)]
  follow: bool,
  /// Seconds between rounds with `--follow`.
  #[arg(long, default_value_t = 1.0)]
  interval: f64,
}

fn main() -> ExitCode {
  let args = Args::parse();
  let path = args.data.join("index.sqlite");
  let journal_root = args.data.join("journal");
  let opened = if args.rebuild {
    rebuild(&path)
  } else {
    open(&path)
  };
  let result = opened.and_then(|mut db| {
    if args.follow {
      let interval = Duration::from_secs_f64(args.interval);
      // Unreadable files are skipped every round; only report each once.
      let mut reported = HashSet::new();
      follow(
        &mut db,
        &journal_root,
        interval,
        || true,
        |round| match round {
          Ok(stats) => {
            if stats.lines > 0 {
              print_counts(stats);
            }
            for (file, why) in &stats.skipped {
              if reported.insert(file.clone()) {
                eprintln!("skipped {file}: {why}");
              }
            }
          }
          Err(e) => eprintln!("index round failed, retrying: {e}"),
        },
      )
    } else {
      index(&mut db, &journal_root).map(|stats| print_stats(&stats))
    }
  });
  match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("index failed: {e}");
      ExitCode::FAILURE
    }
  }
}

fn print_counts(stats: &Stats) {
  eprintln!(
    "{} lines, {} new messages, {} unparsed",
    stats.lines, stats.messages, stats.unparsed
  );
}

fn print_stats(stats: &Stats) {
  print_counts(stats);
  for (file, why) in &stats.skipped {
    eprintln!("skipped {file}: {why}");
  }
}
