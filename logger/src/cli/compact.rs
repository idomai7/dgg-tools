//! Compact closed days of every service's journals into zstd archives.

use {
  crate::{
    compact::{
      DEFAULT_LEVEL, Day, closed_days, compact_day, compact_open_day, open_days,
    },
    dgg::Dgg,
    kick::Kick,
    service::{Root, Service},
  },
  ::clap::Args,
  ::common::time::now_ms,
  ::common::{error, info},
  ::std::{path::Path, process::ExitCode, time::Duration},
};

/// Compact each closed UTC day's live journal files into one zstd archive,
/// verified before the live files are deleted; then the same for the
/// backfill journal, leaving the files of backfill runs still going, and
/// for each Kick channel's.
#[derive(Args)]
pub struct Compact {
  /// Also compact days still open, now: only the files no capture may
  /// still write (an older one's, after an upgrade), into an archive the
  /// rest join once the day is closed.
  #[arg(long)]
  early: bool,
  /// Hours after a day ends before it counts as closed.
  #[arg(long, default_value_t = 1.0)]
  grace_hours: f64,
  /// zstd level, 1 (fast) to 22 (smallest).
  #[arg(long, default_value_t = DEFAULT_LEVEL)]
  level: i32,
  /// Only list the days that would be compacted.
  #[arg(long)]
  dry_run: bool,
  /// Pretend it's this unix ms time (for testing).
  #[arg(long, hide = true)]
  now: Option<i64>,
}

impl Compact {
  /// Compacts each service's journals, in `<data>/journal`: dgg's, then
  /// Kick's.
  pub fn run(self, data: &Path) -> ExitCode {
    let journal = data.join("journal");
    let ok = self.service::<Dgg>(&journal) & self.service::<Kick>(&journal);
    if ok {
      ExitCode::SUCCESS
    } else {
      ExitCode::FAILURE
    }
  }

  /// Compacts each of `S`'s journal roots; whether every day went well.
  fn service<S: Service>(&self, journal: &Path) -> bool {
    match S::roots(&S::dir(journal)) {
      Ok(roots) => {
        let mut ok = true;
        for root in roots {
          ok &= self.compact(&root);
        }
        ok
      }
      Err(e) => {
        error!("compact {} failed: {e}", S::NAME);
        false
      }
    }
  }

  /// Compacts journal `root`; whether every day went well.
  fn compact(&self, root: &Root) -> bool {
    let name = &root.name;
    let grace = Duration::from_secs_f64(self.grace_hours * 3600.0);
    let now = self.now.unwrap_or_else(now_ms);
    let days = match closed_days(root, now, grace) {
      Ok(days) => days,
      Err(e) => {
        error!("compact {name} failed: {e}");
        return false;
      }
    };
    info!("{name}: {} closed days with live files", days.len());
    let open = match self.early {
      true => match open_days(root, now, grace) {
        Ok(open) => open,
        Err(e) => {
          error!("compact {name} failed: {e}");
          return false;
        }
      },
      false => Vec::new(),
    };
    if self.early {
      info!("{name}: {} open days with live files", open.len());
    }
    if self.dry_run {
      for day in days.iter().chain(&open) {
        info!("  {day}");
      }
      return true;
    }
    let mut ok = true;
    for day in &days {
      ok &= report(day, compact_day(root, day, self.level));
    }
    for day in &open {
      ok &= report(day, compact_open_day(root, day, self.level, now));
    }
    ok
  }
}

/// Says how compacting `day` went; whether it went well.
fn report(day: &str, compacted: ::std::io::Result<Option<Day>>) -> bool {
  match compacted {
    Ok(Some(done)) => {
      let archive = &done.archive;
      let ratio = archive.bytes as f64 / archive.compressed.max(1) as f64;
      info!(
        "  {day}: {} files, {} records, {} -> {} bytes \
         ({ratio:.1}x){}{}{}{}{}{}",
        done.files,
        archive.records,
        archive.bytes,
        archive.compressed,
        if done.remerged {
          ", merged into the existing archive"
        } else {
          ""
        },
        note(done.duplicates, "duplicate records dropped"),
        note(done.torn, "torn tails dropped"),
        note(done.grown, "files kept: they grew meanwhile"),
        note(done.busy, "files kept: their backfill is still running"),
        note(done.capturing, "files kept: still being captured"),
      );
      true
    }
    Ok(None) => true,
    Err(e) => {
      error!("  {day}: failed, live files left in place: {e}");
      false
    }
  }
}

fn note(count: u64, what: &str) -> String {
  if count == 0 {
    String::new()
  } else {
    format!(", {count} {what}")
  }
}
