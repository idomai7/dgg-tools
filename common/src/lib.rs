//! Small tools the workspace's crates share, one module each, depending on
//! little beyond `std`: `random` on getrandom, base64 and fastrand.
//!
//! - [`atomic_file`]: replacing a file whole.
//! - [`format`](mod@format): durations, sizes and counts, for people.
//! - [`fs`]: files and directories, beyond `std::fs`.
//! - [`random`]: from the OS, and fast.
//! - [`time`]: unix milliseconds, days, and RFC 3339.

pub mod atomic_file;
pub mod format;
pub mod fs;
pub mod random;
pub mod time;
