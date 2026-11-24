//! Capture Kick chat into the journal.

use {
  super::capture::shutdown,
  crate::{
    capture::{Exit, Handoff, Say, instance_id},
    kick::{
      capture::{Config, run},
      channels,
    },
  },
  ::clap::Args,
  ::common::time::now_ms,
  ::kick_api::{realtime::Options, web::Web},
  ::logger_journal::layout::kick_root,
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
  },
};

/// Capture Kick chat into the journal: every channel in the channel list,
/// each into its own journal root, `kick/<slug>`, read anonymously. Gaps
/// are filled from Kick's chat history. The list is read again every
/// minute: channels added or removed are followed.
#[derive(Args)]
pub struct KickCapture {
  /// The channel list (`kick-channels`, see `kick-ids`).
  #[arg(long)]
  channels: PathBuf,
  /// Host name used in the instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
  /// Seconds a newer instance must be healthy before this one retires.
  #[arg(long, default_value_t = 30)]
  handoff_grace: u64,
  /// Never retire in favour of a newer instance.
  #[arg(long)]
  keep_running: bool,
  /// Don't fill gaps from Kick's chat history.
  #[arg(long)]
  no_fill: bool,
  /// Run as an install's bridge: lines are timestamped and marked, since
  /// no supervisor is there to timestamp them.
  #[arg(long)]
  bridge: bool,
}

impl KickCapture {
  /// Captures into `<data>/journal/kick` until stopped or handed off.
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
    let channels = match channels::load(&self.channels) {
      Ok(channels) if channels.is_empty() => {
        say.error(format_args!("no channels in {}", self.channels.display()));
        return ExitCode::FAILURE;
      }
      Ok(channels) => channels,
      Err(e) => {
        say.error(e);
        return ExitCode::FAILURE;
      }
    };
    let web = if self.no_fill {
      None
    } else {
      let _ = ::rustls::crypto::ring::default_provider().install_default();
      let http = ::reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build();
      match http {
        Ok(http) => Some(Arc::new(Web::new(http, crate::USER_AGENT))),
        Err(e) => {
          say.error(format_args!("can't make a client: {e}"));
          return ExitCode::FAILURE;
        }
      }
    };
    let kick_root = kick_root(&data.join("journal"));
    let slugs: Vec<&str> = channels.iter().map(|c| c.slug.as_str()).collect();
    say.info(format_args!(
      "capturing {} into {} (build {})",
      slugs.join(", "),
      kick_root.display(),
      crate::BUILD
    ));
    let config = Config {
      channels,
      list: Some(self.channels.clone()),
      reread_every: Duration::from_secs(60),
      realtime: Options::default(),
      web,
      kick_root,
      instance,
      started,
      handoff: Handoff {
        grace: Duration::from_secs(self.handoff_grace),
        keep_running: self.keep_running,
        ..Handoff::default()
      },
      restart_after: Duration::from_secs(60),
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
