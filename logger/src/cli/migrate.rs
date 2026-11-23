//! Migrate the journal's archives to records ending at RS LF.

use {
  crate::migrate::{count, migrate},
  ::clap::Args,
  ::std::{path::Path, process::ExitCode},
};

/// Rewrite the archives of `journal 0.1` (a record per line), the journal's
/// and the backfill journal's, as `journal dgg.0.2` (records ending at RS
/// LF), each checked record by record before it replaces the old one, and
/// upgrade the index to match. Waits for, and holds, the index, compact and
/// sync locks, as a supervisor does. Does nothing once it's all done.
#[derive(Args)]
pub struct Migrate {
  /// Convert and check, in a temporary directory, but replace nothing; take
  /// no locks.
  #[arg(long)]
  dry_run: bool,
}

impl Migrate {
  /// Migrates `<data>/journal` and `<data>/index.sqlite`.
  pub fn run(self, data: &Path) -> ExitCode {
    let report = match migrate(data, self.dry_run, &mut |line| {
      eprintln!("{line}");
    }) {
      Ok(report) => report,
      Err(e) => {
        eprintln!("migrate failed: {e}");
        return ExitCode::FAILURE;
      }
    };
    let would = if self.dry_run { "would be " } else { "" };
    eprintln!(
      "{} {would}converted, {} {would}written, {} {would}rewritten",
      count(report.converted, "archive", "archives"),
      count(report.manifests, "manifest", "manifests"),
      count(report.taken, "synced-archives", "synced-archives"),
    );
    if report.failed.is_empty() {
      ExitCode::SUCCESS
    } else {
      eprintln!("{} left as they were", report.failed.len());
      ExitCode::FAILURE
    }
  }
}
