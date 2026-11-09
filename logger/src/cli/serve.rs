//! Serve this host's journal over HTTP, for other hosts.

use {
  crate::serve::{MIN_TOKEN, Tokens, router},
  ::clap::Args,
  ::std::{net::SocketAddr, path::Path, process::ExitCode},
};

/// Serve this host's journal over HTTP, for other hosts: for `sync` to
/// pull from, and (given `--control-token`) for a capture on another host
/// (`capture --peer`) to take over from the ones here.
#[derive(Args)]
pub struct Serve {
  /// Address to listen on. Use 0.0.0.0 to accept other hosts, ideally only
  /// over a VPN or tunnel: the token isn't encrypted.
  #[arg(long, default_value = "127.0.0.1:8788")]
  listen: SocketAddr,
  /// Bearer token for reading the journal (what `sync` needs), at least
  /// 32 characters.
  #[arg(long, env = "LOGGER_TOKEN", hide_env_values = true)]
  token: String,
  /// Bearer token that also lets another host's capture push its status
  /// here, which makes the captures here retire; at least 32 characters,
  /// and not `--token`. Without it, nothing can push.
  #[arg(long, env = "LOGGER_CONTROL_TOKEN", hide_env_values = true)]
  control_token: Option<String>,
}

impl Serve {
  /// Serves `<data>/journal` until stopped.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.serve(data))
  }

  async fn serve(self, data: &Path) -> ExitCode {
    let tokens = Tokens {
      read: self.token,
      control: self.control_token.filter(|token| !token.is_empty()),
    };
    if let Err(why) = tokens.check() {
      eprintln!(
        "{why} (--token or LOGGER_TOKEN, --control-token or          LOGGER_CONTROL_TOKEN; a random {MIN_TOKEN}+ character string will do)"
      );
      return ExitCode::FAILURE;
    }
    let pushes = tokens.control.is_some();
    let listener = match ::tokio::net::TcpListener::bind(self.listen).await {
      Ok(listener) => listener,
      Err(e) => {
        eprintln!("can't listen on {}: {e}", self.listen);
        return ExitCode::FAILURE;
      }
    };
    eprintln!(
      "serving {} on {}{}",
      data.display(),
      self.listen,
      match pushes {
        true => ", taking statuses pushed with the control token",
        false => ", read-only",
      }
    );
    let app = router(data.join("journal"), tokens);
    match ::axum::serve(listener, app).await {
      Ok(()) => ExitCode::SUCCESS,
      Err(e) => {
        eprintln!("serve failed: {e}");
        ExitCode::FAILURE
      }
    }
  }
}
