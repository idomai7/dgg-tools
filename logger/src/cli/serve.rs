//! Serve this host's journal over HTTP, for other hosts.

use {
  crate::serve::router,
  ::clap::Args,
  ::std::{net::SocketAddr, path::Path, process::ExitCode},
};

/// Serve this host's journal over HTTP, for other hosts. For now, this is
/// how a capture on another host (`capture --peer`) hands off to one here.
#[derive(Args)]
pub struct Serve {
  /// Address to listen on. Use 0.0.0.0 to accept other hosts, ideally only
  /// over a VPN or tunnel: the token isn't encrypted.
  #[arg(long, default_value = "127.0.0.1:8788")]
  listen: SocketAddr,
  /// Bearer token clients must send.
  #[arg(long, env = "LOGGER_TOKEN", hide_env_values = true)]
  token: String,
}

impl Serve {
  /// Serves `<data>/journal` until stopped.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.serve(data))
  }

  async fn serve(self, data: &Path) -> ExitCode {
    if self.token.is_empty() {
      eprintln!("serve needs a non-empty --token (or LOGGER_TOKEN)");
      return ExitCode::FAILURE;
    }
    let listener = match ::tokio::net::TcpListener::bind(self.listen).await {
      Ok(listener) => listener,
      Err(e) => {
        eprintln!("can't listen on {}: {e}", self.listen);
        return ExitCode::FAILURE;
      }
    };
    eprintln!("serving {} on {}", data.display(), self.listen);
    let app = router(data.join("journal"), self.token);
    match ::axum::serve(listener, app).await {
      Ok(()) => ExitCode::SUCCESS,
      Err(e) => {
        eprintln!("serve failed: {e}");
        ExitCode::FAILURE
      }
    }
  }
}
