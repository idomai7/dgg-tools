//! Capture dgg chat into the journal.

use {
  crate::capture::{
    Config, Exit, Handoff, Peer, Say, instance_id, now_ms, run,
  },
  ::clap::Args,
  ::dgg_api::{DEFAULT_URL, Options},
  ::std::{path::Path, process::ExitCode, time::Duration},
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
  /// Another host's `serve` (e.g. `http://old-host:8788`) to push this
  /// instance's status to, so the capture there hands off to this one.
  #[arg(long)]
  peer: Option<String>,
  /// Bearer token for `--peer`.
  #[arg(long, env = "LOGGER_PEER_TOKEN", hide_env_values = true)]
  peer_token: Option<String>,
  /// Run as an install's bridge: lines are timestamped and marked, since
  /// no supervisor is there to timestamp them.
  #[arg(long)]
  bridge: bool,
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
    let journal_root = data.join("journal");
    let say = Say {
      instance: instance.clone(),
      bridge: self.bridge,
    };
    say.line(format_args!(
      "capturing into {} (build {})",
      journal_root.display(),
      crate::BUILD
    ));
    let peer = match (self.peer, self.peer_token) {
      (Some(url), Some(token)) if !token.is_empty() => {
        Some(Peer { url, token })
      }
      (Some(_), _) => {
        say.line("--peer needs --peer-token (or LOGGER_PEER_TOKEN)");
        return ExitCode::FAILURE;
      }
      (None, _) => None,
    };
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
      bridge: self.bridge,
    };
    match run(config, shutdown()).await {
      Ok(Exit::Shutdown) => {
        say.line("stopped");
        ExitCode::SUCCESS
      }
      Ok(Exit::HandedOff(successor)) => {
        say.line(format_args!("handed off to {successor}"));
        ExitCode::SUCCESS
      }
      Err(e) => {
        say.line(format_args!("capture failed: {e}"));
        ExitCode::FAILURE
      }
    }
  }
}

/// Completes on Ctrl-C, or on SIGTERM on Unix.
async fn shutdown() {
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
