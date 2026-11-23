//! The settings: `kick-mod.toml`, edited by hand and never written.
//! It holds the app's secret, so it's kept out of git.
//!
//! ```toml
//! [app]
//! client_id = "..."
//! client_secret = "..."
//!
//! [server]
//! listen = "127.0.0.1:8787"
//! ```

use {
  ::serde::Deserialize,
  ::std::{fmt, fs, net::SocketAddr, path::Path},
};

pub const DEFAULT_PATH: &str = "kick-mod.toml";

/// Where the page is served unless `[server]` says otherwise.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  pub app: App,
  #[serde(default)]
  pub server: Server,
}

/// The Kick app, as registered on kick.com.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
  pub client_id: String,
  pub client_secret: String,
}

/// The secret isn't shown.
impl fmt::Debug for App {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.debug_struct("App")
      .field("client_id", &self.client_id)
      .finish_non_exhaustive()
  }
}

#[derive(Clone, Debug, Deserialize)]
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

impl Config {
  pub fn parse(text: &str) -> Result<Self, String> {
    let config: Self = ::toml::from_str(text).map_err(|e| e.to_string())?;
    let listen = config.server.listen;
    if !listen.ip().is_loopback() {
      return Err(format!(
        "server.listen: {listen} isn't a loopback address, and there's no \
         login yet"
      ));
    }
    Ok(config)
  }

  pub fn load(path: &Path) -> Result<Self, String> {
    let text = fs::read_to_string(path)
      .map_err(|e| format!("{}: {e}", path.display()))?;
    Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
  }

  /// The app, for kick-api.
  pub fn app(&self) -> ::kick_api::official::oauth::App {
    ::kick_api::official::oauth::App::new(
      &self.app.client_id,
      &self.app.client_secret,
    )
  }
}
