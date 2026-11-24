//! Migrate the journal and the index to this build's formats.

use {
  crate::migrate::migrate,
  ::clap::Args,
  ::common::{error, info, warn},
  ::std::{path::Path, process::ExitCode},
};

/// Bring the journal and the indexes up to this build's layout and formats,
/// in place: dgg's journals move into `journal/dgg/`, and its index to
/// `index-dgg.sqlite`, leaving live files still being written where they
/// are. Then it checks, saying the version of each kind of file and each
/// index's schema. Waits for, and holds, the index, compact and sync locks,
/// as a supervisor does.
#[derive(Args)]
pub struct Migrate {
  /// Check, but change nothing; take no locks.
  #[arg(long)]
  dry_run: bool,
}

impl Migrate {
  /// Migrates `<data>/journal` and the indexes in `<data>`.
  pub fn run(self, data: &Path) -> ExitCode {
    let report = match migrate(data, self.dry_run, &mut |level, line| {
      eprintln!("{level} {line}");
    }) {
      Ok(report) => report,
      Err(e) => {
        error!("migrate failed: {e}");
        return ExitCode::FAILURE;
      }
    };
    if report.failed.is_empty() {
      let done = match (self.dry_run, report.moved) {
        (true, 0) => "nothing would be migrated".to_owned(),
        (false, 0) => "nothing to migrate".to_owned(),
        (true, n) => format!("{n} files would be moved"),
        (false, n) => format!("{n} files moved"),
      };
      match report.unreadable.len() {
        0 => info!("{done}"),
        n => {
          warn!("{done}; {n} files this build can't read, left as they were")
        }
      }
      ExitCode::SUCCESS
    } else {
      error!("{} left as they were", report.failed.len());
      ExitCode::FAILURE
    }
  }
}
