//! Compact closed days of the journal, and of the backfill journal, into
//! zstd archives.

use {
  crate::{
    capture::now_ms,
    compact::{closed_days, compact_day},
  },
  ::clap::Args,
  ::journal::layout::backfill_root,
  ::std::{path::Path, process::ExitCode, time::Duration},
};

/// Compact each closed UTC day's live journal files into one zstd archive,
/// verified before the live files are deleted; then the same for the
/// backfill journal, leaving the files of backfill runs still going.
#[derive(Args)]
pub struct Compact {
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

impl Compact {
  /// Compacts `<data>/journal`, then its backfill journal.
  pub fn run(self, data: &Path) -> ExitCode {
    let journal_root = data.join("journal");
    let backfill = backfill_root(&journal_root);
    let ok =
      self.compact(&journal_root, "") & self.compact(&backfill, "backfill ");
    if ok {
      ExitCode::SUCCESS
    } else {
      ExitCode::FAILURE
    }
  }

  /// Compacts the journal at `journal_root`, its days labelled with `kind`;
  /// whether every day went well.
  fn compact(&self, journal_root: &Path, kind: &str) -> bool {
    let grace = Duration::from_secs_f64(self.grace_hours * 3600.0);
    let now = self.now.unwrap_or_else(now_ms);
    let days = match closed_days(journal_root, now, grace) {
      Ok(days) => days,
      Err(e) => {
        eprintln!("compact {kind}failed: {e}");
        return false;
      }
    };
    eprintln!("{} closed {kind}days with live files", days.len());
    if self.dry_run {
      for day in &days {
        eprintln!("  {day}");
      }
      return true;
    }
    let mut ok = true;
    for day in &days {
      match compact_day(journal_root, day, self.level) {
        Ok(Some(done)) => {
          let archive = &done.archive;
          let ratio = archive.bytes as f64 / archive.compressed.max(1) as f64;
          eprintln!(
            "  {day}: {} files, {} lines, {} -> {} bytes              ({ratio:.1}x){}{}{}{}{}",
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
            note(done.busy, "files kept: their backfill is still running"),
          );
        }
        Ok(None) => {}
        Err(e) => {
          eprintln!("  {day}: failed, live files left in place: {e}");
          ok = false;
        }
      }
    }
    ok
  }
}

fn note(count: u64, what: &str) -> String {
  if count == 0 {
    String::new()
  } else {
    format!(", {count} {what}")
  }
}
