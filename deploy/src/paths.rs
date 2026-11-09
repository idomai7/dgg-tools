//! Where an install keeps things: one root holding `bin\`, `data\` and
//! `logs\`.

use ::std::{env, path::PathBuf};

/// A user install's root: `%LOCALAPPDATA%\dgg-tools` on Windows,
/// `$XDG_DATA_HOME/dgg-tools` (or `~/.local/share/dgg-tools`) elsewhere.
pub fn user_root() -> PathBuf {
  let base = if cfg!(windows) {
    env::var_os("LOCALAPPDATA").map(PathBuf::from)
  } else {
    env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
      env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
    })
  };
  base.unwrap_or_else(|| PathBuf::from(".")).join("dgg-tools")
}

/// An install's directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
  pub root: PathBuf,
  /// The journal and the index.
  pub data: PathBuf,
  pub logs: PathBuf,
}

impl Paths {
  /// The usual layout under `root`.
  pub fn under(root: PathBuf) -> Self {
    Self {
      data: root.join("data"),
      logs: root.join("logs"),
      root,
    }
  }

  /// Where an install's builds go.
  pub fn bin(&self) -> PathBuf {
    self.root.join("bin")
  }
}
