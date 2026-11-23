//! The dgg chat logger. The commands in `cli`, run as `dgg-logger`
//! subcommands, are thin wrappers over these modules.

pub mod backfill;
pub mod capture;
pub mod cert;
/// This crate's build: its version and the commit it was built from, with
/// `-dirty` if that had uncommitted changes (see `version_info::build`).
pub const BUILD: &str = env!("DGG_TOOLS_BUILD");
/// How the logger names itself to other services: by its codename.
pub const USER_AGENT: &str = concat!("ledgerline/", env!("CARGO_PKG_VERSION"));

pub mod cli;
pub mod compact;
pub mod control;
pub mod gaps;
pub mod index;
pub mod kick;
pub mod manifest;
pub mod migrate;
pub mod serve;
pub mod sync;
pub mod taken;
