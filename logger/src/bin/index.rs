//! Index the journal into SQLite.

use {
  ::clap::Parser,
  ::logger::index::{Error, Stats, follow, index, open, rebuild},
  ::std::{
    collections::HashSet,
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
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
  /// With `--follow`, seconds between summaries of what was indexed (none
  /// is printed if nothing was); 0 prints one for every round.
  #[arg(long, default_value_t = 60.0)]
  report: f64,
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
      let mut reporter = Reporter::new(Duration::from_secs_f64(args.report));
      follow(
        &mut db,
        &journal_root,
        interval,
        || true,
        |round| reporter.round(round),
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

/// Keeps `--follow` quiet: totals are summarised once per period, each
/// skipped file is reported once, and a failure that repeats round after
/// round is reported once, with a note when it clears.
struct Reporter {
  period: Duration,
  since: Instant,
  totals: Stats,
  skipped: HashSet<String>,
  failing: Option<String>,
}

impl Reporter {
  fn new(period: Duration) -> Self {
    Self {
      period,
      since: Instant::now(),
      totals: Stats::default(),
      skipped: HashSet::new(),
      failing: None,
    }
  }

  fn round(&mut self, round: Result<&Stats, &Error>) {
    let stats = match round {
      Ok(stats) => stats,
      Err(e) => {
        let message = e.to_string();
        if self.failing.as_ref() != Some(&message) {
          eprintln!("index round failed, retrying: {message}");
          self.failing = Some(message);
        }
        return;
      }
    };
    if self.failing.take().is_some() {
      eprintln!("index rounds succeeding again");
    }
    for (file, why) in &stats.skipped {
      if self.skipped.insert(file.clone()) {
        eprintln!("skipped {file}: {why}");
      }
    }
    self.totals.lines += stats.lines;
    self.totals.messages += stats.messages;
    self.totals.backfilled += stats.backfilled;
    self.totals.unparsed += stats.unparsed;
    if self.totals.lines > 0 && self.since.elapsed() >= self.period {
      print_counts(&self.totals);
      self.totals = Stats::default();
      self.since = Instant::now();
    }
  }
}

fn print_counts(stats: &Stats) {
  eprintln!(
    "{} lines, {} new messages ({} backfilled), {} unparsed",
    stats.lines, stats.messages, stats.backfilled, stats.unparsed
  );
}

fn print_stats(stats: &Stats) {
  print_counts(stats);
  for (file, why) in &stats.skipped {
    eprintln!("skipped {file}: {why}");
  }
}
