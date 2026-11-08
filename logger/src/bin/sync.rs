//! Pull another host's journal into this one's.

use {
  ::clap::Parser,
  ::logger::{capture::Peer, sync::sync},
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

/// Pull another host's live journal files into this one's, for migrating
/// to a new host or keeping a warm standby. Run `index` afterwards (or keep
/// `index --follow` running) to index what arrived.
#[derive(Parser)]
struct Args {
  /// Data directory; writes `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
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

#[::tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();
  let peer = Peer {
    url: args.peer,
    token: args.peer_token,
  };
  let journal_root = args.data.join("journal");
  let interval = Duration::from_secs_f64(args.interval);
  let mut failing = None;
  loop {
    match sync(&peer, &journal_root).await {
      Ok(stats) => {
        if failing.take().is_some() {
          eprintln!("syncing again");
        }
        if stats.updated > 0 || !args.follow {
          eprintln!(
            "{} files listed, {} updated, {} bytes pulled",
            stats.files, stats.updated, stats.bytes
          );
        }
        for path in &stats.mismatched {
          eprintln!("{path}: shorter on the peer than here; left alone");
        }
      }
      Err(e) if args.follow => {
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
    if !args.follow {
      return ExitCode::SUCCESS;
    }
    ::tokio::time::sleep(interval).await;
  }
}
