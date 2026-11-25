//! Report gaps in what was captured.

use {
  crate::dgg::{
    channels::DESTINY,
    gaps::coverage,
    index::{index, open},
  },
  ::clap::Args,
  ::common::format,
  ::std::{path::Path, process::ExitCode},
};

/// Report stretches when no capture instance was connected, and whether
/// HISTORY replays filled them. Brings the index up to date first.
#[derive(Args)]
pub struct Gaps {
  /// The dgg channel, as `dgg-channels.jsonc` names it.
  #[arg(long, default_value = DESTINY)]
  channel: String,
  /// Only list gaps at least this many seconds long.
  #[arg(long, default_value_t = 0.0)]
  min: f64,
}

impl Gaps {
  /// Reads the channel's journal, in `<data>/journal`, and its index,
  /// `<data>/index-dgg-<channel>.sqlite`.
  pub fn run(self, data: &Path) -> ExitCode {
    let (path, dir) = match super::dgg_channel(data, &self.channel) {
      Ok(found) => found,
      Err(e) => {
        eprintln!("gaps failed: {e}");
        return ExitCode::FAILURE;
      }
    };
    let result = open(&path).and_then(|mut db| {
      index(&mut db, &dir)?;
      Ok(coverage(&db)?)
    });
    let coverage = match result {
      Ok(Some(coverage)) => coverage,
      Ok(None) => {
        println!("nothing captured yet");
        return ExitCode::SUCCESS;
      }
      Err(e) => {
        eprintln!("gaps failed: {e}");
        return ExitCode::FAILURE;
      }
    };
    println!("{}", coverage.summary());
    let min_ms = (self.min * 1000.0) as i64;
    let lines = coverage.lines(min_ms);
    if min_ms > 0 {
      let (shown, ms) = coverage
        .gaps
        .iter()
        .filter(|gap| gap.duration_ms() >= min_ms)
        .fold((0, 0), |(n, ms), gap| (n + 1, ms + gap.duration_ms()));
      println!(
        "{shown} at least {} long, {} in all",
        format::duration(min_ms),
        format::duration(ms)
      );
    }
    for line in lines {
      println!("  {line}");
    }
    ExitCode::SUCCESS
  }
}
