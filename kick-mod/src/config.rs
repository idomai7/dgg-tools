//! The settings: `kick-mod.env.json`, edited by hand and never written, or
//! the same JSON on stdin (see [`::common::config`]). It holds the app's
//! secret and the logger's token, so it's kept out of git; `kick-mod
//! config` shows it without them.
//!
//! ```json
//! {
//!   "app": {"client_id": "...", "client_secret": "..."},
//!   "server": {"listen": "127.0.0.1:8787"},
//!   "logger": {
//!     "url": "https://pi:8788",
//!     "token": "...",
//!     "ca": "-----BEGIN CERTIFICATE-----\n..."
//!   }
//! }
//! ```
//!
//! `server` is optional, and so, for now, is `logger`: the logger's `serve`
//! that kick-mod reads Kick from, with its query token, and the mesh CA's
//! certificate when it's served over HTTPS.

use {
  ::common::config::{self as json, Secret, Source},
  ::serde::{Deserialize, Serialize},
  ::std::net::SocketAddr,
};

pub const DEFAULT_PATH: &str = "kick-mod.env.json";

/// Where the page is served unless `server` says otherwise.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

/// Shown with its secrets as `<secret>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  pub app: App,
  #[serde(default)]
  pub server: Server,
  #[serde(default)]
  pub logger: Option<Logger>,
}

/// The Kick app, as registered on kick.com.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
  pub client_id: String,
  pub client_secret: Secret,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
  /// Where the page is served: a loopback address, while there's no login.
  #[serde(default = "default_listen")]
  pub listen: SocketAddr,
}

impl Default for Server {
  fn default() -> Self {
    Self {
      listen: default_listen(),
    }
  }
}

fn default_listen() -> SocketAddr {
  DEFAULT_LISTEN.parse().unwrap()
}

/// The logger's `serve`, which kick-mod reads Kick's chat from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logger {
  /// Such as `https://pi:8788`.
  pub url: String,
  /// Its query token.
  pub token: Secret,
  /// The mesh CA's certificate (PEM): the only one trusted, and then only
  /// over HTTPS.
  #[serde(default)]
  pub ca: Option<String>,
}

impl Config {
  pub fn parse(text: &str) -> Result<Self, String> {
    Self::check(json::parse(text)?)
  }

  pub fn read(source: Source) -> Result<Self, String> {
    let config = json::read(source)?;
    Self::check(config).map_err(|e| format!("{source}: {e}"))
  }

  fn check(config: Self) -> Result<Self, String> {
    let listen = config.server.listen;
    if !listen.ip().is_loopback() {
      return Err(format!(
        "server.listen: {listen} isn't a loopback address, and there's no \
         login yet"
      ));
    }
    if let Some(logger) = &config.logger {
      logger.check()?;
    }
    Ok(config)
  }

  /// The app, for kick-api.
  pub fn app(&self) -> ::kick_api::official::oauth::App {
    ::kick_api::official::oauth::App::new(
      &self.app.client_id,
      self.app.client_secret.expose(),
    )
  }
}

impl Logger {
  fn check(&self) -> Result<(), String> {
    let url = &self.url;
    let https = url.starts_with("https://");
    if !https && !url.starts_with("http://") {
      return Err(format!("logger.url: {url} isn't an http or https URL"));
    }
    if let Some(ca) = &self.ca {
      if !https {
        return Err(format!(
          "logger.url: {url}: with a CA, the logger is reached over HTTPS \
           only"
        ));
      }
      let certs = ::reqwest::Certificate::from_pem_bundle(ca.as_bytes())
        .map_err(|e| format!("logger.ca: can't read it: {e}"))?;
      if certs.is_empty() {
        return Err("logger.ca: no certificate in it".into());
      }
    }
    Ok(())
  }
}
