//! Small tools the workspace's crates share, one module each, depending on
//! little beyond `std`: `random` on getrandom, base64 and fastrand, and
//! `config` on serde, serde_json and zeroize.
//!
//! - [`atomic_file`]: replacing a file whole.
//! - [`config`]: a command's configuration and secrets, as JSON.
//! - [`format`](mod@format): durations, sizes and counts, for people.
//! - [`fs`]: files and directories, beyond `std::fs`.
//! - [`log`]: log levels, and printing lines at them.
//! - [`powershell`]: running a PowerShell command, on Windows.
//! - [`random`]: from the OS, and fast.
//! - [`temp_dir`]: a directory of a test's own.
//! - [`time`]: unix milliseconds, days, and RFC 3339.

pub mod atomic_file;
pub mod config;
pub mod format;
pub mod fs;
pub mod log;
pub mod powershell;
pub mod random;
pub mod temp_dir;
pub mod time;
