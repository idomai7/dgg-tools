//! Report gaps in what was captured.

use {
  crate::{
    gaps::{coverage, format_duration, format_utc},
    index::{index, open},
  },
  ::clap::Args,
  ::std::{path::Path, process::ExitCode},
};

/// Report stretches when no capture instance was connected, and whether
/// HISTORY replays filled them. Brings the index up to date first.
#[derive(Args)]
pub struct Gaps {
  /// Only list gaps at least this many seconds long.
  #[arg(long, default_value_t = 0.0)]
  min: f64,
}

impl Gaps {
  /// Reads `<data>/journal` and `<data>/index.sqlite`.
  pub fn run(self, data: &Path) -> ExitCode {
    let result = open(&data.join("index.sqlite")).and_then(|mut db| {
      index(&mut db, &data.join("journal"))?;
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
    println!(
      "captured {} to {} UTC ({}), {} gaps",
      format_utc(coverage.first),
      format_utc(coverage.last),
      format_duration(coverage.last - coverage.first),
      coverage.gaps.len()
    );
    let min_ms = (self.min * 1000.0) as i64;
    for gap in coverage
      .gaps
      .iter()
      .filter(|gap| gap.duration_ms() >= min_ms)
    {
      let (instance, connection) = &gap.resumed_by;
      let filled = if gap.covered() {
        "covered by HISTORY".to_owned()
      } else if gap.backfilled {
        "filled by backfill".to_owned()
      } else {
        match gap.history_from {
          Some(from) => {
            format!("NOT covered: HISTORY from {}", format_utc(from))
          }
          None => "NOT covered: no HISTORY".to_owned(),
        }
      };
      println!(
        "  {} +{}  {filled}, {} messages recovered",
        format_utc(gap.start),
        format_duration(gap.duration_ms()),
        gap.recovered,
      );
      println!("    resumed by {instance}#{connection}");
    }
    ExitCode::SUCCESS
  }
}
