//! Small tools the workspace's crates share, one module each, depending on
//! little beyond `std`.
//!
//! - [`atomic_file`]: replacing a file whole.
//! - [`time`]: unix milliseconds, days, and RFC 3339.

pub mod atomic_file;
pub mod time;
