//! Index the journal into SQLite: dgg's into `index.sqlite`, Kick's into
//! `index-kick.sqlite`.

use {
  crate::{
    index::{Error, Pace, Stats, follow, index, open, rebuild},
    kick,
  },
  ::clap::Args,
  ::common::{error, info, warn},
  ::std::{
    collections::HashSet,
    path::Path,
    process::ExitCode,
    thread,
    time::{Duration, Instant},
  },
};

/// Index the journal into SQLite: dgg's, then every Kick channel's into an
/// index of its own.
#[derive(Args)]
pub struct Index {
  /// Delete both indexes and rebuild them from the whole journal.
  #[arg(long, conflicts_with_all = ["rebuild_dgg", "rebuild_kick"])]
  rebuild: bool,
  /// Delete dgg's index and rebuild it from dgg's whole journal.
  #[arg(long)]
  rebuild_dgg: bool,
  /// Delete Kick's index and rebuild it from Kick's whole journal.
  #[arg(long)]
  rebuild_kick: bool,
  /// Keep running, indexing new journal records as they're written: a round
  /// once `--batch` records are waiting, no sooner than `--min-wait` after
  /// the last and no later than `--max-wait`. Kick's index follows on its
  /// own, at the same pace. Stopping it at any point is safe: an
  /// interrupted batch is rolled back and redone.
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
  /// Reads `<data>/journal` into `<data>/index.sqlite`, and Kick's journals
  /// into `<data>/index-kick.sqlite`.
  pub fn run(self, data: &Path) -> ExitCode {
    let journal_root = data.join("journal");
    let dgg_path = data.join("index.sqlite");
    let kick_path = data.join(kick::index::FILE);
    if !self.follow {
      let dgg = self.once(&dgg_path, &journal_root);
      let kick = self.once_kick(&kick_path, &journal_root);
      return match dgg && kick {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
      };
    }
    let pace = Pace {
      look: Duration::from_secs_f64(self.interval),
      batch: self.batch,
      min_wait: Duration::from_secs_f64(self.min_wait),
      max_wait: Duration::from_secs_f64(self.max_wait),
    };
    let period = Duration::from_secs_f64(self.report);
    // Kick's index follows beside dgg's: it failing for good doesn't stop
    // dgg's, nor dgg's it.
    let kick = {
      let (rebuild, journal_root) =
        (self.rebuilds_kick(), journal_root.clone());
      thread::spawn(move || {
        follow_kick(&kick_path, &journal_root, rebuild, pace, period)
      })
    };
    let opened = match self.rebuilds_dgg() {
      true => rebuild(&dgg_path),
      false => open(&dgg_path),
    };
    let mut reporter = Reporter::new("", period);
    let result = opened.and_then(|mut db| {
      follow(
        &mut db,
        &journal_root,
        pace,
        || true,
        |round| reporter.round(round),
      )
    });
    if let Err(e) = result {
      error!("index failed: {e}");
    }
    let _ = kick.join();
    ExitCode::FAILURE
  }

  fn rebuilds_dgg(&self) -> bool {
    self.rebuild || self.rebuild_dgg
  }

  fn rebuilds_kick(&self) -> bool {
    self.rebuild || self.rebuild_kick
  }

  /// Brings dgg's index at `path` up to date once; whether it went well.
  fn once(&self, path: &Path, journal_root: &Path) -> bool {
    let opened = match self.rebuilds_dgg() {
      true => rebuild(path),
      false => open(path),
    };
    report_once("", opened.and_then(|mut db| index(&mut db, journal_root)))
  }

  /// Brings Kick's index at `path` up to date once; whether it went well.
  fn once_kick(&self, path: &Path, journal_root: &Path) -> bool {
    let opened = match self.rebuilds_kick() {
      true => kick::index::rebuild(path),
      false => kick::index::open(path),
    };
    let indexed =
      opened.and_then(|mut db| kick::index::index(&mut db, journal_root));
    report_once("kick: ", indexed)
  }
}

/// Says how a one-off run went, each line after `prefix`; whether it went
/// well.
fn report_once<S: Tally>(prefix: &str, indexed: Result<S, Error>) -> bool {
  match indexed {
    Ok(stats) => {
      info!("{prefix}{}", stats.summary());
      for (file, why) in stats.skipped() {
        warn!("{prefix}skipped {file}: {why}");
      }
      true
    }
    Err(e) => {
      error!("{prefix}index failed: {e}");
      false
    }
  }
}

/// Keeps Kick's index at `path` up to date, until it fails for good.
fn follow_kick(
  path: &Path,
  journal_root: &Path,
  rebuild: bool,
  pace: Pace,
  period: Duration,
) {
  let opened = match rebuild {
    true => kick::index::rebuild(path),
    false => kick::index::open(path),
  };
  let mut reporter = Reporter::new("kick: ", period);
  let result = opened.and_then(|mut db| {
    kick::index::follow(
      &mut db,
      journal_root,
      pace,
      || true,
      |round| reporter.round(round),
    )
  });
  if let Err(e) = result {
    error!("kick: index failed: {e}");
  }
}

/// What a round's stats have in common, for [`Reporter`].
trait Tally: Default {
  fn records(&self) -> u64;
  fn skipped(&self) -> &[(String, String)];
  /// Adds `other`'s counts to these.
  fn add(&mut self, other: &Self);
  /// The counts, as a line.
  fn summary(&self) -> String;
}

impl Tally for Stats {
  fn records(&self) -> u64 {
    self.records
  }

  fn skipped(&self) -> &[(String, String)] {
    &self.skipped
  }

  fn add(&mut self, other: &Self) {
    self.records += other.records;
    self.messages += other.messages;
    self.backfilled += other.backfilled;
    self.unparsed += other.unparsed;
  }

  fn summary(&self) -> String {
    format!(
      "{} records, {} new messages ({} backfilled), {} unparsed",
      self.records, self.messages, self.backfilled, self.unparsed
    )
  }
}

impl Tally for kick::index::Stats {
  fn records(&self) -> u64 {
    self.records
  }

  fn skipped(&self) -> &[(String, String)] {
    &self.skipped
  }

  fn add(&mut self, other: &Self) {
    self.records += other.records;
    self.messages += other.messages;
    self.recovered += other.recovered;
    self.deletions += other.deletions;
    self.bans += other.bans;
    self.unparsed += other.unparsed;
  }

  fn summary(&self) -> String {
    format!(
      "{} records, {} new messages ({} from history), {} deletions, {} \
       bans and unbans, {} unparsed",
      self.records,
      self.messages,
      self.recovered,
      self.deletions,
      self.bans,
      self.unparsed
    )
  }
}

/// Keeps `--follow` quiet: totals are summarised once per period, each
/// skipped file is reported once, and a failure that repeats round after
/// round is reported once, with a note when it clears. Each line starts
/// with `prefix`.
struct Reporter<S> {
  prefix: &'static str,
  period: Duration,
  since: Instant,
  totals: S,
  skipped: HashSet<String>,
  failing: Option<String>,
}

impl<S: Tally> Reporter<S> {
  fn new(prefix: &'static str, period: Duration) -> Self {
    Self {
      prefix,
      period,
      since: Instant::now(),
      totals: S::default(),
      skipped: HashSet::new(),
      failing: None,
    }
  }

  fn round(&mut self, round: Result<&S, &Error>) {
    let prefix = self.prefix;
    let stats = match round {
      Ok(stats) => stats,
      Err(e) => {
        let message = e.to_string();
        if self.failing.as_ref() != Some(&message) {
          warn!("{prefix}index round failed, retrying: {message}");
          self.failing = Some(message);
        }
        return;
      }
    };
    if self.failing.take().is_some() {
      info!("{prefix}index rounds succeeding again");
    }
    for (file, why) in stats.skipped() {
      if self.skipped.insert(file.clone()) {
        warn!("{prefix}skipped {file}: {why}");
      }
    }
    self.totals.add(stats);
    if self.totals.records() > 0 && self.since.elapsed() >= self.period {
      info!("{prefix}{}", self.totals.summary());
      self.totals = S::default();
      self.since = Instant::now();
    }
  }
}
