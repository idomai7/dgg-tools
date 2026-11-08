//! Compact closed days of the journal into zstd archives.

use {
  ::clap::Parser,
  ::logger::{
    capture::now_ms,
    compact::{closed_days, compact_day},
  },
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

/// Compact each closed UTC day's live journal files into one zstd archive,
/// verified before the live files are deleted.
#[derive(Parser)]
struct Args {
  /// Data directory; compacts `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  /// Hours after a day ends before it counts as closed.
  #[arg(long, default_value_t = 1.0)]
  grace_hours: f64,
  /// zstd level, 1 (fast) to 22 (smallest).
  #[arg(long, default_value_t = 19)]
  level: i32,
  /// Only list the days that would be compacted.
  #[arg(long)]
  dry_run: bool,
  /// Pretend it's this unix ms time (for testing).
  #[arg(long, hide = true)]
  now: Option<i64>,
}

fn main() -> ExitCode {
  let args = Args::parse();
  let journal_root = args.data.join("journal");
  let grace = Duration::from_secs_f64(args.grace_hours * 3600.0);
  let now = args.now.unwrap_or_else(now_ms);
  let days = match closed_days(&journal_root, now, grace) {
    Ok(days) => days,
    Err(e) => {
      eprintln!("compact failed: {e}");
      return ExitCode::FAILURE;
    }
  };
  eprintln!("{} closed days with live files", days.len());
  if args.dry_run {
    for day in &days {
      eprintln!("  {day}");
    }
    return ExitCode::SUCCESS;
  }
  let mut failed = false;
  for day in &days {
    match compact_day(&journal_root, day, args.level) {
      Ok(Some(done)) => {
        let archive = &done.archive;
        let ratio = archive.bytes as f64 / archive.compressed.max(1) as f64;
        eprintln!(
          "  {day}: {} files, {} lines, {} -> {} bytes ({ratio:.1}x){}{}{}{}",
          done.files,
          archive.lines,
          archive.bytes,
          archive.compressed,
          if done.remerged {
            ", merged into the existing archive"
          } else {
            ""
          },
          note(done.duplicates, "duplicate lines dropped"),
          note(done.torn, "torn tails dropped"),
          note(done.grown, "files kept: they grew meanwhile"),
        );
      }
      Ok(None) => {}
      Err(e) => {
        eprintln!("  {day}: failed, live files left in place: {e}");
        failed = true;
      }
    }
  }
  if failed {
    ExitCode::FAILURE
  } else {
    ExitCode::SUCCESS
  }
}

fn note(count: u64, what: &str) -> String {
  if count == 0 {
    String::new()
  } else {
    format!(", {count} {what}")
  }
}
