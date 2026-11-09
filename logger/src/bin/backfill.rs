//! Fill gaps that HISTORY didn't cover from rustlesearch.dev or
//! linkers.ooo.

use {
  ::clap::{Parser, ValueEnum},
  ::logger::{
    backfill::{
      Options, Source, linkers::Linkers, run, rustlesearch::Rustlesearch,
    },
    capture::{instance_id, now_ms},
    gaps::{Gap, coverage, format_duration, format_utc},
    index::{index, open},
  },
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

/// Fill gaps that HISTORY didn't cover from rustlesearch.dev or
/// linkers.ooo.
#[derive(Parser)]
struct Args {
  /// Data directory: reads `<data>/index.sqlite`, writes `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  /// Where to fetch from.
  #[arg(long, value_enum, default_value_t = SourceName::Rustlesearch)]
  source: SourceName,
  /// An `lk_session` cookie from a logged-in linkers.ooo session, which
  /// lifts its rate limit.
  #[arg(long, env = "LINKERS_SESSION", hide_env_values = true)]
  linkers_session: Option<String>,
  /// Most requests to make in this run [default: 50 for rustlesearch, 200
  /// for linkers].
  #[arg(long)]
  max_requests: Option<u64>,
  /// Stop when the rate limit has fewer than this many requests left
  /// [default: 10 for rustlesearch, 2 for linkers].
  #[arg(long)]
  reserve: Option<u64>,
  /// Seconds to fetch past each end of a gap, for clock differences.
  #[arg(long, default_value_t = 5.0)]
  margin: f64,
  /// Seconds between requests [default: 1 for rustlesearch, 1.5 for
  /// linkers].
  #[arg(long)]
  delay: Option<f64>,
  /// Host name used in this run's instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
  /// Only list the gaps that would be fetched.
  #[arg(long)]
  dry_run: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceName {
  /// 200 requests a day, 100 messages each, to the millisecond.
  Rustlesearch,
  /// 60 requests a minute (unlimited with `--linkers-session`), 500
  /// messages each, to the second; only since 2026-01-06.
  Linkers,
}

impl SourceName {
  /// Default `(max_requests, reserve, delay)`.
  fn defaults(self) -> (u64, u64, f64) {
    match self {
      Self::Rustlesearch => (50, 10, 1.0),
      Self::Linkers => (200, 2, 1.5),
    }
  }
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
      .filter(|gap| !gap.filled())
      .collect(),
    Err(e) => {
      eprintln!("backfill failed: {e}");
      return ExitCode::FAILURE;
    }
  };
  eprintln!("{} gaps not yet filled", gaps.len());
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
  let (max_requests, reserve, delay) = args.source.defaults();
  let options = Options {
    journal_root,
    instance: format!("backfill-{}", instance_id(&host, now_ms())),
    margin: Duration::from_secs_f64(args.margin),
    max_requests: args.max_requests.unwrap_or(max_requests),
    reserve: args.reserve.unwrap_or(reserve),
    delay: Duration::from_secs_f64(args.delay.unwrap_or(delay)),
  };
  match args.source {
    SourceName::Rustlesearch => {
      let base = ::logger::backfill::rustlesearch::BASE_URL;
      fetch(&Rustlesearch::new(base, "Destinygg"), &gaps, &options).await
    }
    SourceName::Linkers => {
      let base = ::logger::backfill::linkers::BASE_URL;
      let source = Linkers::new(base, args.linkers_session);
      fetch(&source, &gaps, &options).await
    }
  }
}

async fn fetch(
  source: &impl Source,
  gaps: &[Gap],
  options: &Options,
) -> ExitCode {
  match run(source, gaps, options).await {
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
