//! Pull another host's journal into this one's.

use {
  super::ConfigFrom,
  crate::{capture::Peer, sync::sync},
  ::clap::Args,
  ::common::{config::Secret, error, info, warn},
  ::serde::Deserialize,
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
  },
};

/// Pull another host's live journal files into this one's, for migrating
/// to a new host or keeping a warm standby. Run `index` afterwards (or keep
/// `index --follow` running) to index what arrived.
#[derive(Args)]
pub struct Sync {
  /// The other host's `serve`, e.g. `https://203.0.113.5:8788`.
  #[arg(long)]
  peer: String,
  /// The mesh CA's certificate: reaches --peer over HTTPS, trusting that
  /// CA alone.
  #[arg(long)]
  peer_ca: Option<PathBuf>,
  /// Its config: `{"token": "<--peer's read token>"}`.
  #[command(flatten)]
  config: ConfigFrom,
  /// Keep syncing, every `--interval` seconds.
  #[arg(long)]
  follow: bool,
  /// Seconds between rounds with `--follow`.
  #[arg(long, default_value_t = 5.0)]
  interval: f64,
}

/// What `sync` reads with `--config-file` or `--config-stdin`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  /// The peer's read token.
  pub token: Secret,
}

impl Sync {
  /// Pulls into `<data>/journal`, once or with `--follow` until stopped.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.sync(data))
  }

  async fn sync(self, data: &Path) -> ExitCode {
    let config: Config = match self.config.require(r#"{"token": "..."}"#) {
      Ok(config) => config,
      Err(e) => {
        error!("{e}");
        return ExitCode::FAILURE;
      }
    };
    let ca = match super::read_peer_ca(self.peer_ca.as_deref(), &self.peer) {
      Ok(ca) => ca,
      Err(e) => {
        error!("{e}");
        return ExitCode::FAILURE;
      }
    };
    let peer = Peer {
      url: self.peer,
      token: config.token.expose().to_owned(),
      ca,
    };
    let journal_root = data.join("journal");
    let interval = Duration::from_secs_f64(self.interval);
    let mut failing = None;
    loop {
      match sync(&peer, &journal_root).await {
        Ok(stats) => {
          if failing.take().is_some() {
            info!("syncing again");
          }
          if stats.updated > 0 || !self.follow {
            info!(
              "{} files listed, {} updated, {} bytes pulled",
              stats.files, stats.updated, stats.bytes
            );
          }
          if stats.installed + stats.merging > 0 {
            info!(
              "{} archives installed, {} written out for compaction to merge",
              stats.installed, stats.merging
            );
          }
          for path in &stats.mismatched {
            warn!("{path}: shorter on the peer than here; left alone");
          }
        }
        Err(e) if self.follow => {
          if failing.as_ref() != Some(&e) {
            warn!("sync round failed, retrying: {e}");
            failing = Some(e);
          }
        }
        Err(e) => {
          error!("sync failed: {e}");
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
