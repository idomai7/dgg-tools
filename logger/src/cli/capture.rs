//! Capture dgg chat into the journal.

use {
  super::ConfigFrom,
  crate::{
    capture::{Exit, Handoff, Peer, Say, instance_id},
    control,
    dgg::{
      Dgg,
      capture::{Config, run},
      channels::DESTINY,
    },
    service::Service,
  },
  ::clap::Args,
  ::common::{config::Secret, time::now_ms},
  ::dgg_api::chat::{DEFAULT_URL, Options},
  ::serde::Deserialize,
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
  },
};

/// Capture dgg chat into the journal.
#[derive(Args)]
pub struct Capture {
  /// Host name used in the instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
  /// Chat socket URL.
  #[arg(long, default_value = DEFAULT_URL)]
  url: String,
  /// Don't send an Origin header.
  #[arg(long)]
  no_origin: bool,
  /// Seconds a newer instance must be healthy before this one retires.
  #[arg(long, default_value_t = 30)]
  handoff_grace: u64,
  /// Never retire in favour of a newer instance.
  #[arg(long)]
  keep_running: bool,
  /// Another host's `serve` (e.g. `https://203.0.113.5:8788`) to push
  /// this instance's status to, so the capture there hands off to this one.
  #[arg(long)]
  peer: Option<String>,
  /// The mesh CA's certificate: reaches --peer over HTTPS, trusting that
  /// CA alone.
  #[arg(long, requires = "peer")]
  peer_ca: Option<PathBuf>,
  /// Its config, with `--peer`: `{"token": "<--peer's control token>"}`.
  #[command(flatten)]
  config: ConfigFrom,
  /// Run as an install's bridge: lines are timestamped and marked, since
  /// no supervisor is there to timestamp them.
  #[arg(long)]
  bridge: bool,
}

/// What `capture --peer` reads with `--config-file` or `--config-stdin`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
  /// The control token of `--peer`'s `serve`.
  pub token: Secret,
}

impl Capture {
  /// Captures into `<data>/journal` until stopped or handed off.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.capture(data))
  }

  async fn capture(self, data: &Path) -> ExitCode {
    let host = self.host.unwrap_or_else(|| {
      ::gethostname::gethostname().to_string_lossy().into_owned()
    });
    let started = now_ms();
    let instance = instance_id(&host, started);
    let mut chat = Options {
      url: self.url,
      ..Options::default()
    };
    if self.no_origin {
      chat.origin = None;
    }
    let dir = Dgg::dir(&data.join("journal"));
    // destiny.gg's, the one channel captured for now.
    let journal_root = Dgg::channel_dir(&dir, DESTINY);
    let say = Say {
      instance: instance.clone(),
      bridge: self.bridge,
    };
    let peer = match self.peer {
      Some(url) => {
        let config = self.config.require::<PeerConfig>(r#"{"token": "..."}"#);
        let ca = super::read_peer_ca(self.peer_ca.as_deref(), &url);
        match (config, ca) {
          (Ok(config), Ok(ca)) => Some(Peer {
            url,
            token: config.token.expose().to_owned(),
            ca,
          }),
          (Err(e), _) => {
            say.error(format_args!("--peer {e}"));
            return ExitCode::FAILURE;
          }
          (_, Err(e)) => {
            say.error(e);
            return ExitCode::FAILURE;
          }
        }
      }
      None => match self.config.read::<PeerConfig>() {
        Ok(None) => None,
        Ok(Some(_)) | Err(_) => {
          say.error("a config is only for --peer's token");
          return ExitCode::FAILURE;
        }
      },
    };
    say.info(format_args!(
      "capturing into {} (build {})",
      journal_root.display(),
      crate::BUILD
    ));
    let config = Config {
      chat,
      journal_root,
      instance,
      started,
      handoff: Handoff {
        grace: Duration::from_secs(self.handoff_grace),
        keep_running: self.keep_running,
        ..Handoff::default()
      },
      peer,
      old_control: Some(control::dir(&dir)),
      bridge: self.bridge,
    };
    match run(config, shutdown()).await {
      Ok(Exit::Shutdown) => {
        say.info("stopped");
        ExitCode::SUCCESS
      }
      Ok(Exit::HandedOff(successor)) => {
        say.info(format_args!("handed off to {successor}"));
        ExitCode::SUCCESS
      }
      Err(e) => {
        say.error(format_args!("capture failed: {e}"));
        ExitCode::FAILURE
      }
    }
  }
}

/// Completes on Ctrl-C, or on SIGTERM on Unix.
pub(super) async fn shutdown() {
  #[cfg(unix)]
  {
    use ::tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).unwrap();
    ::tokio::select! {
      _ = ::tokio::signal::ctrl_c() => {}
      _ = term.recv() => {}
    }
  }
  #[cfg(not(unix))]
  {
    let _ = ::tokio::signal::ctrl_c().await;
  }
}
