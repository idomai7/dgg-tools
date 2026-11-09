//! Fetch messages from rustlesearch.dev or linkers.ooo into the journal's
//! `backfill/` directory: gaps that HISTORY didn't cover, or with `--from`,
//! a stretch of history.

use {
  ::clap::{Parser, ValueEnum},
  ::logger::{
    backfill::{
      Options, Source, linkers::Linkers, run, rustlesearch::Rustlesearch,
    },
    capture::{instance_id, now_ms},
    gaps::{coverage, format_duration, format_utc, unfetched},
    index::{index, open},
  },
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

/// Fetch messages from rustlesearch.dev or linkers.ooo into the journal's
/// `backfill/` directory, kept apart from what was captured live.
///
/// By default, fills gaps that HISTORY didn't cover. With `--from`, imports
/// history instead: everything between `--from` and `--to` that this source
/// hasn't been fetched for yet, newest first. A history import is meant to
/// run for weeks at a gentle pace; stopping it at any point is safe, and
/// running it again picks up where it left off.
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
  /// Import history from this UTC date or time (`2016-01-01` or
  /// `2016-01-01T12:00:00Z`) instead of filling gaps.
  #[arg(long, value_parser = parse_time)]
  from: Option<i64>,
  /// With `--from`, import history up to this UTC date or time [default:
  /// now].
  #[arg(long, value_parser = parse_time, requires = "from")]
  to: Option<i64>,
  /// Most requests to make in this run [default: 50 for rustlesearch, 200
  /// for linkers; unlimited with `--from`].
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
  /// linkers; 10 with `--from`, which takes linkers.ooo's whole archive
  /// about two months].
  #[arg(long)]
  delay: Option<f64>,
  /// Failed requests in a row to retry, with growing waits, before giving
  /// up [default: 3; 20 with `--from`].
  #[arg(long)]
  retries: Option<u32>,
  /// Seconds between progress lines [default: 600; none without `--from`].
  #[arg(long)]
  report: Option<f64>,
  /// Host name used in this run's instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
  /// Only list what would be fetched.
  #[arg(long)]
  dry_run: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceName {
  /// 200 requests a day, 100 messages each, to the millisecond.
  Rustlesearch,
  /// 60 requests a minute (unlimited with `--linkers-session`), 500
  /// messages each. Its own log, since 2026-01-06, is to the second; before
  /// that it relays the rustle archive, journaled as `linkers-rustlesearch`
  /// and `linkers-overrustle`.
  Linkers,
}

impl SourceName {
  fn name(self) -> &'static str {
    match self {
      Self::Rustlesearch => "rustlesearch",
      Self::Linkers => "linkers",
    }
  }

  /// Default `(max_requests, reserve, delay)` for filling gaps.
  fn defaults(self) -> (u64, u64, f64) {
    match self {
      Self::Rustlesearch => (50, 10, 1.0),
      Self::Linkers => (200, 2, 1.5),
    }
  }
}

/// Unix ms of `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS[.fff]Z`, in UTC.
fn parse_time(text: &str) -> Result<i64, String> {
  let full = if text.contains('T') {
    text.to_owned()
  } else {
    format!("{text}T00:00:00Z")
  };
  ::logger::backfill::rustlesearch::parse_timestamp(&full)
    .ok_or_else(|| format!("not a UTC date or time: {text:?}"))
}

#[::tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();
  let history = args.from.is_some();
  let journal_root = args.data.join("journal");
  let margin = (args.margin * 1000.0) as i64;
  if let Err(e) = ::std::fs::create_dir_all(&args.data) {
    eprintln!("backfill failed: {}: {e}", args.data.display());
    return ExitCode::FAILURE;
  }
  let found = open(&args.data.join("index.sqlite")).and_then(|mut db| {
    index(&mut db, &journal_root)?;
    let ranges = match args.from {
      Some(from) => {
        let to = args.to.unwrap_or_else(now_ms);
        unfetched(&db, args.source.name(), from, to)?
      }
      None => coverage(&db)?
        .map(|coverage| coverage.gaps)
        .unwrap_or_default()
        .into_iter()
        .filter(|gap| !gap.filled())
        .map(|gap| (gap.start - margin, gap.end + margin))
        .collect(),
    };
    Ok(ranges)
  });
  let ranges = match found {
    Ok(ranges) => ranges,
    Err(e) => {
      eprintln!("backfill failed: {e}");
      return ExitCode::FAILURE;
    }
  };
  let what = if history { "ranges" } else { "gaps" };
  eprintln!("{} {what} not yet fetched", ranges.len());
  for &(from, to) in &ranges {
    eprintln!("  {} +{}", format_utc(from), format_duration(to - from));
  }
  if args.dry_run || ranges.is_empty() {
    return ExitCode::SUCCESS;
  }

  let host = args.host.unwrap_or_else(|| {
    ::gethostname::gethostname().to_string_lossy().into_owned()
  });
  let (max_requests, reserve, delay) = args.source.defaults();
  let (max_requests, delay, retries, report) = if history {
    (u64::MAX, 10.0, 20, Some(600.0))
  } else {
    (max_requests, delay, 3, None)
  };
  let options = Options {
    journal_root,
    instance: format!("backfill-{}", instance_id(&host, now_ms())),
    max_requests: args.max_requests.unwrap_or(max_requests),
    reserve: args.reserve.unwrap_or(reserve),
    delay: Duration::from_secs_f64(args.delay.unwrap_or(delay)),
    retries: args.retries.unwrap_or(retries),
    report: args.report.or(report).map(Duration::from_secs_f64),
  };
  match args.source {
    SourceName::Rustlesearch => {
      let base = ::logger::backfill::rustlesearch::BASE_URL;
      let source = Rustlesearch::new(base, "Destinygg");
      fetch(&source, &ranges, &options, what).await
    }
    SourceName::Linkers => {
      let base = ::logger::backfill::linkers::BASE_URL;
      let source = Linkers::new(base, args.linkers_session);
      fetch(&source, &ranges, &options, what).await
    }
  }
}

async fn fetch(
  source: &impl Source,
  ranges: &[(i64, i64)],
  options: &Options,
  what: &str,
) -> ExitCode {
  match run(source, ranges, options).await {
    Ok(stats) => {
      eprintln!(
        "{} requests ({} retried), {} messages fetched, {} of {} {what} \
         filled{}",
        stats.requests,
        stats.retried,
        stats.messages,
        stats.filled,
        ranges.len(),
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
