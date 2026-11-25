//! Capture a dgg channel's chat into the journal.

use {
  super::ConfigFrom,
  crate::{
    capture::{Exit, Handoff, Peer, REMOVED_EXIT, Say, instance_id},
    control,
    dgg::{
      Dgg,
      capture::{Config, run},
      channels::{self, Channel, DESTINY},
    },
    service::Service,
  },
  ::clap::Args,
  ::common::{config::Secret, time::now_ms},
  ::dgg_api::chat::Options,
  ::serde::Deserialize,
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
  },
};

/// Capture one dgg channel's chat into its journal, `dgg/<name>`. A list
/// given is read again every minute: a changed URL or Origin is connected to, and
/// if the channel's removed, this capture stops, exiting 3.
#[derive(Args)]
pub struct Capture {
  /// The channel list (`dgg-channels.jsonc`): with none, or no file,
  /// destiny.gg's alone.
  #[arg(long)]
  channels: Option<PathBuf>,
  /// The channel to capture, by its name: one in the list.
  #[arg(long)]
  channel: String,
  /// Host name used in the instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
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
  /// Captures into `<data>/journal/dgg/<name>` until stopped, handed off
  /// or removed from the list.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.capture(data))
  }

  async fn capture(self, data: &Path) -> ExitCode {
    let host = self.host.unwrap_or_else(|| {
      ::gethostname::gethostname().to_string_lossy().into_owned()
    });
    let started = now_ms();
    let instance = instance_id(&host, started);
    let say = Say {
      instance: instance.clone(),
      bridge: self.bridge,
    };
    let listed = match &self.channels {
      Some(list) => channels::load(list),
      None => Ok(vec![Channel::destiny()]),
    };
    let listed = match listed {
      Ok(listed) => listed,
      Err(e) => {
        say.error(e);
        return ExitCode::FAILURE;
      }
    };
    let Some(channel) = listed.into_iter().find(|c| c.name == self.channel)
    else {
      let list = match &self.channels {
        Some(list) => list.display().to_string(),
        None => "the channels (destiny.gg's alone, with no --channels)".into(),
      };
      say.error(format_args!("{} isn't in {list}", self.channel));
      return ExitCode::FAILURE;
    };
    let dir = Dgg::dir(&data.join("journal"));
    let journal_root = Dgg::channel_dir(&dir, &channel.name);
    // destiny.gg's capture is the one there was before dgg's journals were
    // by channel, whose old instances look in `dgg/control/`.
    let old_control = (channel.name == DESTINY).then(|| control::dir(&dir));
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
      "capturing {} into {} (build {})",
      channel.name,
      journal_root.display(),
      crate::BUILD
    ));
    let config = Config {
      channel,
      list: self.channels.clone(),
      reread_every: Duration::from_secs(60),
      chat: Options::default(),
      journal_root,
      instance,
      started,
      handoff: Handoff {
        grace: Duration::from_secs(self.handoff_grace),
        keep_running: self.keep_running,
        ..Handoff::default()
      },
      peer,
      old_control,
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
      Ok(Exit::Removed) => {
        say.info(format_args!("stopped: {}", self.channel));
        ExitCode::from(REMOVED_EXIT)
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
