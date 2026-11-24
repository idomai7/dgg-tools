//! Migrate the journal and the index to this build's formats.

use {
  crate::migrate::migrate,
  ::clap::Args,
  ::std::{path::Path, process::ExitCode},
};

/// Bring the journal and the index up to this build's formats, in place.
/// There's nothing to migrate yet: it only checks, saying the version of
/// each kind of file, the index's schema and the captures running. Waits
/// for, and holds, the index, compact and sync locks, as a supervisor does.
#[derive(Args)]
pub struct Migrate {
  /// Check, but change nothing; take no locks.
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
    if report.failed.is_empty() {
      let unreadable = match report.unreadable.len() {
        0 => String::new(),
        n => format!("; {n} files this build can't read, left as they were"),
      };
      if self.dry_run {
        eprintln!("nothing would be migrated{unreadable}");
      } else {
        eprintln!("nothing to migrate{unreadable}");
      }
      ExitCode::SUCCESS
    } else {
      eprintln!("{} left as they were", report.failed.len());
      ExitCode::FAILURE
    }
  }
}
