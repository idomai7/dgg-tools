//! Running the logger as a long-lived service: a supervisor that keeps its
//! programs running, and (to come) installing it as a scheduled task or a
//! Windows service, and showing its status.

/// This crate's build, as `dgg-logger --version` shows it: its version and
/// the commit it was built from (see `version_info::build`).
pub const BUILD: &str = env!("DGG_TOOLS_BUILD");

pub mod install;
pub mod lifeline;
pub mod log;
pub mod paths;
#[cfg(windows)]
mod powershell;
pub mod processes;
pub mod status;
pub mod supervise;
