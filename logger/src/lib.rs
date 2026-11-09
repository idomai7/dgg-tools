//! The dgg chat logger. The commands in `cli`, run as `dgg-logger`
//! subcommands, are thin wrappers over these modules.

pub mod backfill;
pub mod capture;
/// This build: the commit it was built from, with `-dirty` if it had
/// uncommitted changes (see `version_info::commit`).
pub const BUILD: &str = env!("DGG_TOOLS_BUILD");

pub mod cli;
pub mod compact;
pub mod control;
pub mod gaps;
pub mod index;
pub mod manifest;
pub mod serve;
pub mod sync;
