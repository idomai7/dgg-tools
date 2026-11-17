//! The logger's commands, as `dgg-logger` subcommands: each one's arguments
//! (all but the data directory, which the caller supplies) and what it does.

pub mod backfill;
pub mod capture;
pub mod cert;
pub mod compact;
pub mod gaps;
pub mod index;
pub mod serve;
pub mod sync;

pub use {
  backfill::Backfill, capture::Capture, cert::Cert, compact::Compact,
  gaps::Gaps, index::Index, serve::Serve, sync::Sync,
};

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

/// Runs an async command on a new runtime.
fn block_on(
  future: impl ::std::future::Future<Output = ::std::process::ExitCode>,
) -> ::std::process::ExitCode {
  match ::tokio::runtime::Runtime::new() {
    Ok(runtime) => runtime.block_on(future),
    Err(e) => {
      eprintln!("can't start the async runtime: {e}");
      ::std::process::ExitCode::FAILURE
    }
  }
}
