//! Fill gaps that HISTORY didn't cover from rustlesearch.dev.

use {
  ::clap::Parser,
  ::logger::{
    backfill::{
      Options, run,
      rustlesearch::{BASE_URL, Rustlesearch},
    },
    capture::{instance_id, now_ms},
    gaps::{coverage, format_duration, format_utc},
    index::{index, open},
  },
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

/// Fill gaps that HISTORY didn't cover from rustlesearch.dev, which allows
/// 200 requests a day (100 messages each).
#[derive(Parser)]
struct Args {
  /// Data directory: reads `<data>/index.sqlite`, writes `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  /// Most requests to make in this run.
  #[arg(long, default_value_t = 50)]
  max_requests: u64,
  /// Stop when the rate limit has fewer than this many requests left.
  #[arg(long, default_value_t = 10)]
  reserve: u64,
  /// Seconds to fetch past each end of a gap, for clock differences.
  #[arg(long, default_value_t = 5.0)]
  margin: f64,
  /// Seconds between requests.
  #[arg(long, default_value_t = 1.0)]
  delay: f64,
  /// Host name used in this run's instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
  /// Only list the gaps that would be fetched.
  #[arg(long)]
  dry_run: bool,
}

#[::tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();
  let journal_root = args.data.join("journal");
  let found = open(&args.data.join("index.sqlite")).and_then(|mut db| {
    index(&mut db, &journal_root)?;
    Ok(coverage(&db)?)
  });
  let gaps: Vec<_> = match found {
    Ok(coverage) => coverage
      .map(|coverage| coverage.gaps)
      .unwrap_or_default()
      .into_iter()
      .filter(|gap| !gap.covered())
      .collect(),
    Err(e) => {
      eprintln!("backfill failed: {e}");
      return ExitCode::FAILURE;
    }
  };
  eprintln!("{} gaps not covered by HISTORY", gaps.len());
  for gap in &gaps {
    eprintln!(
      "  {} +{}",
      format_utc(gap.start),
      format_duration(gap.duration_ms())
    );
  }
  if args.dry_run || gaps.is_empty() {
    return ExitCode::SUCCESS;
  }

  let host = args.host.unwrap_or_else(|| {
    ::gethostname::gethostname().to_string_lossy().into_owned()
  });
  let options = Options {
    journal_root,
    instance: format!("backfill-{}", instance_id(&host, now_ms())),
    margin: Duration::from_secs_f64(args.margin),
    max_requests: args.max_requests,
    reserve: args.reserve,
    delay: Duration::from_secs_f64(args.delay),
  };
  let source = Rustlesearch::new(BASE_URL, "Destinygg");
  match run(&source, &gaps, &options).await {
    Ok(stats) => {
      eprintln!(
        "{} requests, {} messages fetched, {} of {} gaps filled{}",
        stats.requests,
        stats.messages,
        stats.filled,
        gaps.len(),
        if stats.skipped > 0 {
          format!(", {} skipped (bad nick)", stats.skipped)
        } else {
          String::new()
        }
      );
      if let Some(why) = stats.stopped {
        eprintln!("stopped early: {why}");
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("backfill failed: {e}");
      ExitCode::FAILURE
    }
  }
}
