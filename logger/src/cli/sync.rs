//! Pull another host's journal into this one's.

use {
  crate::{capture::Peer, sync::sync},
  ::clap::Args,
  ::std::{path::Path, process::ExitCode, time::Duration},
};

/// Pull another host's live journal files into this one's, for migrating
/// to a new host or keeping a warm standby. Run `index` afterwards (or keep
/// `index --follow` running) to index what arrived.
#[derive(Args)]
pub struct Sync {
  /// The other host's `serve`, e.g. `http://old-host:8788`.
  #[arg(long)]
  peer: String,
  /// Bearer token for `--peer`.
  #[arg(long, env = "LOGGER_PEER_TOKEN", hide_env_values = true)]
  peer_token: String,
  /// Keep syncing, every `--interval` seconds.
  #[arg(long)]
  follow: bool,
  /// Seconds between rounds with `--follow`.
  #[arg(long, default_value_t = 5.0)]
  interval: f64,
}

impl Sync {
  /// Pulls into `<data>/journal`, once or with `--follow` until stopped.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.sync(data))
  }

  async fn sync(self, data: &Path) -> ExitCode {
    let peer = Peer {
      url: self.peer,
      token: self.peer_token,
    };
    let journal_root = data.join("journal");
    let interval = Duration::from_secs_f64(self.interval);
    let mut failing = None;
    loop {
      match sync(&peer, &journal_root).await {
        Ok(stats) => {
          if failing.take().is_some() {
            eprintln!("syncing again");
          }
          if stats.updated > 0 || !self.follow {
            eprintln!(
              "{} files listed, {} updated, {} bytes pulled",
              stats.files, stats.updated, stats.bytes
            );
          }
          if stats.installed + stats.merging > 0 {
            eprintln!(
              "{} archives installed, {} written out for compaction to merge",
              stats.installed, stats.merging
            );
          }
          for path in &stats.mismatched {
            eprintln!("{path}: shorter on the peer than here; left alone");
          }
        }
        Err(e) if self.follow => {
          if failing.as_ref() != Some(&e) {
            eprintln!("sync round failed, retrying: {e}");
            failing = Some(e);
          }
        }
        Err(e) => {
          eprintln!("sync failed: {e}");
          return ExitCode::FAILURE;
        }
      }
      if !self.follow {
        return ExitCode::SUCCESS;
      }
      ::tokio::time::sleep(interval).await;
    }
  }
}
