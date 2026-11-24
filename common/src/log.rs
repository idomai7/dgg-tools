//! Log levels: each line a program logs starts with one, padded to the same
//! width, so a supervisor can keep it and a reader can colour it:
//!
//! ```text
//! INFO  capturing into data/journal
//! WARN  waiting for compact.lock: another supervisor has it
//! ERROR migrate failed: …
//! ```
//!
//! [`info!`](crate::info), [`warn!`](crate::warn) and
//! [`error!`](crate::error) print such a line to stderr; [`Level::split`]
//! takes one apart.

use ::std::fmt;

/// How much a log line asks for attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
  /// What's happening.
  Info,
  /// Something is wrong, but work goes on.
  Warn,
  /// Something failed or stopped.
  Error,
}

impl Level {
  /// Its name in log lines.
  pub fn name(self) -> &'static str {
    match self {
      Self::Info => "INFO",
      Self::Warn => "WARN",
      Self::Error => "ERROR",
    }
  }

  /// The level `line` starts with, and the rest of it; `None` if it starts
  /// with none.
  pub fn split(line: &str) -> Option<(Self, &str)> {
    let (name, rest) = line.split_once(' ')?;
    let level = [Self::Info, Self::Warn, Self::Error]
      .into_iter()
      .find(|level| level.name() == name)?;
    Some((level, rest.trim_start_matches(' ')))
  }
}

impl fmt::Display for Level {
  /// Its name, padded to the width of the longest.
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "{:<5}", self.name())
  }
}

/// Prints a line at [`Level::Info`] to stderr, formatted as `format!`.
#[macro_export]
macro_rules! info {
  ($($arg:tt)*) => {
    ::std::eprintln!(
      "{} {}",
      $crate::log::Level::Info,
      ::std::format_args!($($arg)*)
    )
  };
}

/// Prints a line at [`Level::Warn`] to stderr, formatted as `format!`.
#[macro_export]
macro_rules! warn {
  ($($arg:tt)*) => {
    ::std::eprintln!(
      "{} {}",
      $crate::log::Level::Warn,
      ::std::format_args!($($arg)*)
    )
  };
}

/// Prints a line at [`Level::Error`] to stderr, formatted as `format!`.
#[macro_export]
macro_rules! error {
  ($($arg:tt)*) => {
    ::std::eprintln!(
      "{} {}",
      $crate::log::Level::Error,
      ::std::format_args!($($arg)*)
    )
  };
}
