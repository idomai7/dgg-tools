//! The dgg chat logger. The commands in `cli` (run as `dgg-logger`
//! subcommands, and by the binaries in `src/bin/`) are thin wrappers over
//! these modules.

pub mod backfill;
pub mod capture;
pub mod cli;
pub mod compact;
pub mod control;
pub mod gaps;
pub mod index;
pub mod manifest;
pub mod serve;
pub mod sync;
