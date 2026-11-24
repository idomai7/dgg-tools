//! Index each service's journals into SQLite: dgg's into
//! `index-dgg.sqlite`, Kick's into `index-kick.sqlite`.

use {
  crate::{
    dgg::Dgg,
    index::{self, Error, Pace, Tally},
    kick::Kick,
    service::Service,
  },
  ::clap::Args,
  ::common::{error, info, warn},
  ::logger_journal::Variant,
  ::rusqlite::Connection,
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
  /// Reads each service's journals, under `<data>/journal`, into its index
  /// in `<data>`.
  pub fn run(self, data: &Path) -> ExitCode {
    let journal_root = data.join("journal");
    if !self.follow {
      let dgg = once::<Dgg>(data, &journal_root, self.rebuilds_dgg());
      let kick = once::<Kick>(data, &journal_root, self.rebuilds_kick());
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
      let (data, journal_root) = (data.to_owned(), journal_root.clone());
      let rebuild = self.rebuilds_kick();
      thread::spawn(move || {
        keep_up::<Kick>(&data, &journal_root, rebuild, pace, period)
      })
    };
    keep_up::<Dgg>(data, &journal_root, self.rebuilds_dgg(), pace, period);
    let _ = kick.join();
    ExitCode::FAILURE
  }

  fn rebuilds_dgg(&self) -> bool {
    self.rebuild || self.rebuild_dgg
  }

  fn rebuilds_kick(&self) -> bool {
    self.rebuild || self.rebuild_kick
  }
}

/// Each line about service `S` starts with this: none for dgg's, whose
/// lines came first.
fn prefix<S: Service>() -> &'static str {
  match S::VARIANT {
    Variant::Dgg => "",
    Variant::Kick => "kick: ",
  }
}

/// `S`'s index in `data`, rebuilt if asked.
fn opened<S: Service>(data: &Path, rebuild: bool) -> Result<Connection, Error> {
  let path = data.join(S::INDEX);
  match rebuild {
    true => index::rebuild::<S>(&path),
    false => S::open_index(&path),
  }
}

/// Brings `S`'s index up to date once, saying how it went; whether it went
/// well.
fn once<S: Service>(data: &Path, journal_root: &Path, rebuild: bool) -> bool {
  let prefix = prefix::<S>();
  let indexed = opened::<S>(data, rebuild)
    .and_then(|mut db| S::index(&mut db, journal_root));
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

/// Keeps `S`'s index up to date, until it fails for good.
fn keep_up<S: Service>(
  data: &Path,
  journal_root: &Path,
  rebuild: bool,
  pace: Pace,
  period: Duration,
) {
  let mut reporter = Reporter::<S::Stats>::new(prefix::<S>(), period);
  let result = opened::<S>(data, rebuild).and_then(|mut db| {
    index::follow::<S>(
      &mut db,
      journal_root,
      pace,
      || true,
      |round| reporter.round(round),
    )
  });
  if let Err(e) = result {
    error!("{}index failed: {e}", prefix::<S>());
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
