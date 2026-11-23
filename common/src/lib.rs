//! Small tools the workspace's crates share, one module each, depending on
//! little beyond `std`.
//!
//! - [`atomic_file`]: replacing a file whole.
//! - [`format`]: durations, sizes and counts, for people.
//! - [`time`]: unix milliseconds, days, and RFC 3339.

pub mod atomic_file;
pub mod format;
pub mod time;
