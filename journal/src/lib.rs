//! The logger's journal: append-only line files that are the source of truth
//! for everything captured from dgg chat.
//!
//! Live files live at `<root>/live/YYYY-MM-DD/<instance>.journal`, one per
//! capture instance per UTC day, so overlapping instances never share a file
//! (see [`layout`]).
//!
//! # Versions
//!
//! The first line of every file is `journal <major>.<minor>` (see
//! [`version`]); the rest of the file is in that version's line format. Each
//! major version has its own module describing its structure, with child
//! modules for encoding and decoding, so old and new formats coexist:
//!
//! - [`v1`]: the current format.
//!
//! A reader accepts any minor of a major it has a module for. So a minor bump
//! may only make changes old readers tolerate: new kinds, which they report
//! as [`DecodeError::Unknown`] and skip. Anything else is a major bump, and
//! readers without that major's module refuse the file.

pub mod layout;
pub mod reader;
pub mod record;
pub mod v1;
pub mod version;
pub mod writer;

pub use {
  reader::{Entry, ReadError, Reader},
  record::{DecodeError, Record},
  v1 as current,
  version::Version,
  writer::Writer,
};

pub(crate) fn digits(s: &str) -> bool {
  !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
