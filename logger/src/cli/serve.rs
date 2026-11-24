//! Serve this host's journal over HTTPS, for other hosts.

use {
  crate::{
    cert,
    serve::{self, MIN_TOKEN, Tokens, router},
  },
  ::clap::Args,
  ::common::{error, info, warn},
  ::std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
  },
};

/// Serve this host's journal over HTTPS, for other hosts: for `sync` to
/// pull from, and (given `--control-token`) for a capture on another host
/// (`capture --peer`) to take over from the ones here.
#[derive(Args)]
pub struct Serve {
  /// Address to listen on. Use 0.0.0.0 to accept other hosts, with
  /// --tls-cert and --tls-key: without them, tokens aren't encrypted.
  #[arg(long, default_value = "127.0.0.1:8788")]
  listen: SocketAddr,
  /// This node's certificate, from `cert node`: serves HTTPS with it.
  #[arg(long, requires = "tls_key")]
  tls_cert: Option<PathBuf>,
  /// The certificate's key.
  #[arg(long, requires = "tls_cert")]
  tls_key: Option<PathBuf>,
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
      error!(
        "{why} (--token or LOGGER_TOKEN, --control-token or \
         LOGGER_CONTROL_TOKEN; a random {MIN_TOKEN}+ character string will \
         do)"
      );
      return ExitCode::FAILURE;
    }
    let tls = match (&self.tls_cert, &self.tls_key) {
      (Some(cert), Some(key)) => match tls(cert, key) {
        Ok(tls) => Some(tls),
        Err(e) => {
          error!("{e}");
          return ExitCode::FAILURE;
        }
      },
      _ => None,
    };
    let pushes = tokens.control.is_some();
    let listener = match ::tokio::net::TcpListener::bind(self.listen).await {
      Ok(listener) => listener,
      Err(e) => {
        error!("can't listen on {}: {e}", self.listen);
        return ExitCode::FAILURE;
      }
    };
    if tls.is_none() && !self.listen.ip().is_loopback() {
      warn!(
        "serving plain HTTP to other hosts, so tokens travel in \
         the clear; give --tls-cert and --tls-key"
      );
    }
    info!(
      "serving {} on {} over {}{}",
      data.display(),
      self.listen,
      match tls {
        Some(_) => "HTTPS",
        None => "HTTP",
      },
      match pushes {
        true => ", taking statuses pushed with the control token",
        false => ", read-only",
      }
    );
    let app = router(data.join("journal"), tokens);
    match serve::run(listener, app, tls).await {
      Ok(()) => ExitCode::SUCCESS,
      Err(e) => {
        error!("serve failed: {e}");
        ExitCode::FAILURE
      }
    }
  }
}

/// The TLS config serving `cert` with `key`.
fn tls(cert: &Path, key: &Path) -> Result<::rustls::ServerConfig, String> {
  let read = |path: &Path| {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
  };
  cert::server_config(&read(cert)?, &::zeroize::Zeroizing::new(read(key)?))
    .map_err(|e| format!("{}: {e}", cert.display()))
}
