//! Index each service's journals into SQLite: each dgg channel's into
//! `index-dgg-<name>.sqlite`, each Kick channel's into
//! `index-kick-<slug>.sqlite`.

use {
  crate::{
    dgg::Dgg,
    index::{self, Error, Pace, Tally},
    kick::Kick,
    service::{Indexed, Service},
  },
  ::clap::Args,
  ::common::{error, info, warn},
  ::rusqlite::Connection,
  ::std::{
    collections::HashSet,
    path::Path,
    process::ExitCode,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
  },
};

/// Index the journal into SQLite: each channel's into an index of its own,
/// dgg's then Kick's.
#[derive(Args)]
pub struct Index {
  /// Delete every index and rebuild them from the whole journal.
  #[arg(long, conflicts_with_all = ["rebuild_dgg", "rebuild_kick"])]
  rebuild: bool,
  /// Delete every dgg channel's index and rebuild it from the channel's
  /// whole journal.
  #[arg(long)]
  rebuild_dgg: bool,
  /// Delete every Kick channel's index and rebuild it from the channel's
  /// whole journal.
  #[arg(long)]
  rebuild_kick: bool,
  /// Keep running, indexing new journal records as they're written: a round
  /// once `--batch` records are waiting, no sooner than `--min-wait` after
  /// the last and no later than `--max-wait`. Each index follows on its
  /// own, at the same pace, a new channel's from the look it's first seen
  /// at. Stopping it at any point is safe: an interrupted batch is
  /// rolled back and redone.
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
  /// Reads each service's journals, under `<data>/journal`, into its
  /// indexes in `<data>`.
  pub fn run(self, data: &Path) -> ExitCode {
    let journal = data.join("journal");
    if !self.follow {
      let dgg = once::<Dgg>(data, &journal, self.rebuilds_dgg());
      let kick = once::<Kick>(data, &journal, self.rebuilds_kick());
      return match dgg && kick {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
      };
    }
    let following = Following {
      pace: Pace {
        look: Duration::from_secs_f64(self.interval),
        batch: self.batch,
        min_wait: Duration::from_secs_f64(self.min_wait),
        max_wait: Duration::from_secs_f64(self.max_wait),
      },
      period: Duration::from_secs_f64(self.report),
      started: HashSet::new(),
      threads: Vec::new(),
      failing: None,
    };
    following.run(data, &journal, self.rebuilds_dgg(), self.rebuilds_kick())
  }

  fn rebuilds_dgg(&self) -> bool {
    self.rebuild || self.rebuild_dgg
  }

  fn rebuilds_kick(&self) -> bool {
    self.rebuild || self.rebuild_kick
  }
}

/// `--follow`: a thread keeping each index up to date, started at the
/// first look it's there.
struct Following {
  pace: Pace,
  period: Duration,
  /// Each index started, by its file.
  started: HashSet<String>,
  threads: Vec<JoinHandle<()>>,
  /// Why the last look for indexes failed, if it did.
  failing: Option<String>,
}

impl Following {
  /// Looks for new indexes every look, until every one started has failed
  /// for good.
  fn run(
    mut self,
    data: &Path,
    journal: &Path,
    rebuild_dgg: bool,
    rebuild_kick: bool,
  ) -> ExitCode {
    loop {
      self.start::<Dgg>(data, journal, rebuild_dgg);
      self.start::<Kick>(data, journal, rebuild_kick);
      if !self.threads.is_empty()
        && self.threads.iter().all(JoinHandle::is_finished)
      {
        return ExitCode::FAILURE;
      }
      thread::sleep(self.pace.look);
    }
  }

  /// Starts following each of `S`'s indexes not yet started, rebuilding it
  /// first if asked. One failing for good doesn't stop the others.
  fn start<S: Service>(&mut self, data: &Path, journal: &Path, rebuild: bool) {
    let indexes = match S::indexes(&S::dir(journal)) {
      Ok(indexes) => indexes,
      Err(e) => {
        let message = format!("{}: can't look for indexes: {e}", S::NAME);
        if self.failing.as_ref() != Some(&message) {
          warn!("{message}");
          self.failing = Some(message);
        }
        return;
      }
    };
    for indexed in indexes {
      if !self.started.insert(indexed.file.clone()) {
        continue;
      }
      let (data, pace, period) = (data.to_owned(), self.pace, self.period);
      self.threads.push(thread::spawn(move || {
        keep_up::<S>(&data, &indexed, rebuild, pace, period)
      }));
    }
  }
}

/// What's said of a live file the index forgot, gone from the journal.
fn forgot(file: &str) -> String {
  format!(
    "{file} is gone from the journal; its checkpoint and unparsed rows      are dropped (if it wasn't compacted, what the index hadn't reached      is lost)"
  )
}

/// Each line about `indexed` starts with this.
fn prefix(indexed: &Indexed) -> String {
  format!("{}: ", indexed.name)
}

/// `S`'s index `indexed`, in `data`, rebuilt if asked.
fn opened<S: Service>(
  data: &Path,
  indexed: &Indexed,
  rebuild: bool,
) -> Result<Connection, Error> {
  let path = data.join(&indexed.file);
  match rebuild {
    true => index::rebuild::<S>(&path),
    false => S::open_index(&path),
  }
}

/// Brings each of `S`'s indexes up to date once, saying how it went;
/// whether every one went well.
fn once<S: Service>(data: &Path, journal: &Path, rebuild: bool) -> bool {
  let indexes = match S::indexes(&S::dir(journal)) {
    Ok(indexes) => indexes,
    Err(e) => {
      error!("{}: can't look for indexes: {e}", S::NAME);
      return false;
    }
  };
  let mut ok = true;
  for indexed in indexes {
    let prefix = prefix(&indexed);
    let done = opened::<S>(data, &indexed, rebuild)
      .and_then(|mut db| S::index(&mut db, &indexed.dir));
    match done {
      Ok(stats) => {
        info!("{prefix}{}", stats.summary());
        for (file, why) in stats.skipped() {
          warn!("{prefix}skipped {file}: {why}");
        }
        for file in stats.forgotten() {
          warn!("{prefix}{}", forgot(file));
        }
      }
      Err(e) => {
        error!("{prefix}index failed: {e}");
        ok = false;
      }
    }
  }
  ok
}

/// Keeps `S`'s index `indexed` up to date, until it fails for good.
fn keep_up<S: Service>(
  data: &Path,
  indexed: &Indexed,
  rebuild: bool,
  pace: Pace,
  period: Duration,
) {
  let mut reporter = Reporter::<S::Stats>::new(prefix(indexed), period);
  let result = opened::<S>(data, indexed, rebuild).and_then(|mut db| {
    index::follow::<S>(
      &mut db,
      &indexed.dir,
      pace,
      || true,
      |round| reporter.round(round),
    )
  });
  if let Err(e) = result {
    error!("{}index failed: {e}", prefix(indexed));
  }
}

/// Keeps `--follow` quiet: totals are summarised once per period, each
/// skipped file is reported once, and a failure that repeats round after
/// round is reported once, with a note when it clears. Each line starts
/// with `prefix`.
struct Reporter<S> {
  prefix: String,
  period: Duration,
  since: Instant,
  totals: S,
  skipped: HashSet<String>,
  failing: Option<String>,
}

impl<S: Tally> Reporter<S> {
  fn new(prefix: String, period: Duration) -> Self {
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
    let prefix = &self.prefix;
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
    for file in stats.forgotten() {
      warn!("{prefix}{}", forgot(file));
    }
    self.totals.add(stats);
    if self.totals.records() > 0 && self.since.elapsed() >= self.period {
      info!("{prefix}{}", self.totals.summary());
      self.totals = S::default();
      self.since = Instant::now();
    }
  }
}
