//! The logger's commands, as `dgg-logger` subcommands: each one's arguments
//! (all but the data directory, which the caller supplies) and what it does.

pub mod backfill;
pub mod capture;
pub mod compact;
pub mod gaps;
pub mod index;
pub mod serve;
pub mod sync;

pub use {
  backfill::Backfill, capture::Capture, compact::Compact, gaps::Gaps,
  index::Index, serve::Serve, sync::Sync,
};

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
