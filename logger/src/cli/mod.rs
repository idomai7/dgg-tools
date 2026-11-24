//! The logger's commands, as `dgg-logger` subcommands: each one's arguments
//! (all but the data directory, which the caller supplies) and what it does.

pub mod backfill;
pub mod capture;
pub mod cert;
pub mod compact;
pub mod gaps;
pub mod index;
pub mod kick_capture;
pub mod kick_ids;
pub mod migrate;
pub mod serve;
pub mod sync;

pub use {
  ::common::error, backfill::Backfill, capture::Capture, cert::Cert,
  compact::Compact, gaps::Gaps, index::Index, kick_capture::KickCapture,
  kick_ids::KickIds, migrate::Migrate, serve::Serve, sync::Sync,
};

/// Where a command's config and secrets are, as JSON (see
/// [`common::config`]): never in a flag's value or the environment.
#[derive(::clap::Args)]
pub struct ConfigFrom {
  /// Read this command's config and secrets from this JSON file.
  #[arg(long, conflicts_with = "config_stdin")]
  config_file: Option<::std::path::PathBuf>,
  /// Read this command's config and secrets from stdin, as JSON.
  #[arg(long)]
  config_stdin: bool,
}

impl ConfigFrom {
  /// The config, if one was given.
  pub fn read<T: for<'de> ::serde::Deserialize<'de>>(
    &self,
  ) -> Result<Option<T>, String> {
    use ::common::config::{Source, read};
    match (&self.config_file, self.config_stdin) {
      (Some(path), _) => read(Source::File(path)).map(Some),
      (None, true) => read(Source::Stdin).map(Some),
      (None, false) => Ok(None),
    }
  }

  /// The config, which must be given: `shape` says what it holds.
  pub fn require<T: for<'de> ::serde::Deserialize<'de>>(
    &self,
    shape: &str,
  ) -> Result<T, String> {
    self.read()?.ok_or_else(|| {
      format!("needs --config-file or --config-stdin, with {shape}")
    })
  }
}

/// The CA certificate at `peer_ca`, if given, checked, for reaching `url`,
/// which must then be HTTPS.
fn read_peer_ca(
  peer_ca: Option<&::std::path::Path>,
  url: &str,
) -> Result<Option<String>, String> {
  let Some(path) = peer_ca else {
    return Ok(None);
  };
  if !url.starts_with("https://") {
    return Err(format!(
      "{url}: with --peer-ca, a peer is reached over HTTPS only"
    ));
  }
  let pem = ::std::fs::read_to_string(path)
    .map_err(|e| format!("{}: {e}", path.display()))?;
  crate::cert::roots(&pem).map_err(|e| format!("{}: {e}", path.display()))?;
  Ok(Some(pem))
}

/// The CA certificate `pem`, from a config, checked, for reaching `url`,
/// which must then be HTTPS.
fn peer_ca(pem: String, url: &str) -> Result<String, String> {
  if !url.starts_with("https://") {
    return Err(format!(
      "{url}: with a CA, a peer is reached over HTTPS only"
    ));
  }
  crate::cert::roots(&pem).map_err(|e| format!("the config's ca: {e}"))?;
  Ok(pem)
}

/// Runs an async command on a new runtime.
fn block_on(
  future: impl ::std::future::Future<Output = ::std::process::ExitCode>,
) -> ::std::process::ExitCode {
  match ::tokio::runtime::Runtime::new() {
    Ok(runtime) => runtime.block_on(future),
    Err(e) => {
      error!("can't start the async runtime: {e}");
      ::std::process::ExitCode::FAILURE
    }
  }
}
