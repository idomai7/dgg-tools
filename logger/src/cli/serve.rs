//! Serve this host's journal over HTTPS, for other hosts.

use {
  super::ConfigFrom,
  crate::{
    cert,
    serve::{self, MIN_TOKEN, Tokens, router},
  },
  ::clap::Args,
  ::common::{config::Secret, error, info, warn},
  ::serde::Deserialize,
  ::std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
  },
};

/// Serve this host's journal over HTTPS, for other hosts: for `sync` to
/// pull from, (given a control token) for a capture on another host
/// (`capture --peer`) to take over from the ones here, and Kick's history
/// and live chat for kick-mod.
#[derive(Args)]
pub struct Serve {
  /// Address to listen on. Use 0.0.0.0 to accept other hosts, with TLS
  /// (--tls-cert and --tls-key, or the config's `tls`): without it, tokens
  /// aren't encrypted.
  #[arg(long, default_value = "127.0.0.1:8788")]
  listen: SocketAddr,
  /// This node's certificate, from `cert node`: serves HTTPS with it.
  #[arg(long, requires = "tls_key")]
  tls_cert: Option<PathBuf>,
  /// The certificate's key.
  #[arg(long, requires = "tls_cert")]
  tls_key: Option<PathBuf>,
  /// Its config: its bearer tokens, `{"tokens": {"read": "...",
  /// "control": "...", "query": "..."}}`, `control` and `query` optional;
  /// and optionally, in place of --tls-cert and --tls-key, `"tls":
  /// {"cert": "<PEM>", "key": "<PEM>"}`.
  #[command(flatten)]
  config: ConfigFrom,
}

/// What `serve` reads with `--config-file` or `--config-stdin`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  pub tokens: ConfigTokens,
  #[serde(default)]
  pub tls: Option<ConfigTls>,
}

/// What `serve` serves HTTPS with.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTls {
  /// This node's certificate (PEM), from `cert node`.
  pub cert: String,
  /// Its key (PEM).
  pub key: Secret,
}

/// `serve`'s bearer tokens, each at least [`MIN_TOKEN`] characters, and no
/// two the same.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTokens {
  /// Reads the journal (what `sync` needs).
  pub read: Secret,
  /// Also lets another host's capture push its status here, which makes
  /// the captures here retire. Without it, nothing can push.
  pub control: Option<Secret>,
  /// Reads only Kick's history and live chat, for kick-mod.
  pub query: Option<Secret>,
}

impl Serve {
  /// Serves `<data>/journal` until stopped.
  pub fn run(self, data: &Path) -> ExitCode {
    super::block_on(self.serve(data))
  }

  async fn serve(self, data: &Path) -> ExitCode {
    let shape =
      r#"{"tokens": {"read": "...", "control": "...", "query": "..."}}"#;
    let given = self.tls_cert.is_some();
    let config: Config = match self.config.require(shape) {
      Ok(config) => config,
      Err(e) => {
        error!("{e}");
        return ExitCode::FAILURE;
      }
    };
    let expose =
      |token: Option<Secret>| token.map(|token| token.expose().to_owned());
    let tokens = Tokens {
      read: config.tokens.read.expose().to_owned(),
      control: expose(config.tokens.control),
      query: expose(config.tokens.query),
    };
    if let Err(why) = tokens.check() {
      error!("{why} (a random {MIN_TOKEN}+ character string will do)");
      return ExitCode::FAILURE;
    }
    let tls = match (config.tls, &self.tls_cert, &self.tls_key) {
      (Some(_), _, _) if given => {
        Err("--tls-cert and --tls-key or the config's tls, not both".into())
      }
      (Some(tls), _, _) => {
        let key = ::zeroize::Zeroizing::new(tls.key.expose().to_owned());
        cert::server_config(&tls.cert, &key)
          .map(Some)
          .map_err(|e| format!("the config's tls: {e}"))
      }
      (None, Some(cert), Some(key)) => read_tls(cert, key).map(Some),
      (None, _, _) => Ok(None),
    };
    let tls = match tls {
      Ok(tls) => tls,
      Err(e) => {
        error!("{e}");
        return ExitCode::FAILURE;
      }
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
         the clear; give it TLS"
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

/// The TLS config serving the certificate in `cert` with the key in `key`.
fn read_tls(cert: &Path, key: &Path) -> Result<::rustls::ServerConfig, String> {
  let read = |path: &Path| {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
  };
  cert::server_config(&read(cert)?, &::zeroize::Zeroizing::new(read(key)?))
    .map_err(|e| format!("{}: {e}", cert.display()))
}
