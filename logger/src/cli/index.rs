//! Index the journal into SQLite.

use {
  crate::index::{Error, Pace, Stats, follow, index, open, rebuild},
  ::clap::Args,
  ::common::{error, info, warn},
  ::std::{
    collections::HashSet,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
  },
};

/// Index the journal into SQLite.
#[derive(Args)]
pub struct Index {
  /// Delete the index and rebuild it from the whole journal.
  #[arg(long)]
  rebuild: bool,
  /// Keep running, indexing new journal records as they're written: a round
  /// once `--batch` records are waiting, no sooner than `--min-wait` after
  /// the last and no later than `--max-wait`. Stopping it at any point is
  /// safe: an interrupted batch is rolled back and redone.
  #[arg(long)]
  follow: bool,
  /// With `--follow`, seconds between looks at what's waiting.
  #[arg(long, default_value_t = 1.0)]
  interval: f64,
  /// With `--follow`, the records waiting that make a round due.
  #[arg(long, default_value_t = 500)]
  batch: u64,
  /// With `--follow`, the fewest seconds between rounds.
  #[arg(long, default_value_t = 60.0)]
  min_wait: f64,
  /// With `--follow`, the most seconds between rounds, however few records
  /// are waiting.
  #[arg(long, default_value_t = 600.0)]
  max_wait: f64,
  /// With `--follow`, seconds between summaries of what was indexed (none
  /// is printed if nothing was); 0 prints one for every round.
  #[arg(long, default_value_t = 60.0)]
  report: f64,
}

impl Index {
  /// Reads `<data>/journal` into `<data>/index.sqlite`.
  pub fn run(self, data: &Path) -> ExitCode {
    let path = data.join("index.sqlite");
    let journal_root = data.join("journal");
    let opened = if self.rebuild {
      rebuild(&path)
    } else {
      open(&path)
    };
    let result = opened.and_then(|mut db| {
      if self.follow {
        let pace = Pace {
          look: Duration::from_secs_f64(self.interval),
          batch: self.batch,
          min_wait: Duration::from_secs_f64(self.min_wait),
          max_wait: Duration::from_secs_f64(self.max_wait),
        };
        let mut reporter = Reporter::new(Duration::from_secs_f64(self.report));
        follow(
          &mut db,
          &journal_root,
          pace,
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
        error!("index failed: {e}");
        ExitCode::FAILURE
      }
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
          warn!("index round failed, retrying: {message}");
          self.failing = Some(message);
        }
        return;
      }
    };
    if self.failing.take().is_some() {
      info!("index rounds succeeding again");
    }
    for (file, why) in &stats.skipped {
      if self.skipped.insert(file.clone()) {
        warn!("skipped {file}: {why}");
      }
    }
    self.totals.records += stats.records;
    self.totals.messages += stats.messages;
    self.totals.backfilled += stats.backfilled;
    self.totals.unparsed += stats.unparsed;
    if self.totals.records > 0 && self.since.elapsed() >= self.period {
      print_counts(&self.totals);
      self.totals = Stats::default();
      self.since = Instant::now();
    }
  }
}

fn print_counts(stats: &Stats) {
  info!(
    "{} records, {} new messages ({} backfilled), {} unparsed",
    stats.records, stats.messages, stats.backfilled, stats.unparsed
  );
}

fn print_stats(stats: &Stats) {
  print_counts(stats);
  for (file, why) in &stats.skipped {
    warn!("skipped {file}: {why}");
  }
}
