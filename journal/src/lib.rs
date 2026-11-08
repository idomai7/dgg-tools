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
//! [`record::Version`]); the rest of the file is in that version's line
//! format. Each major version has its own module under [`record`] describing
//! its structure, encoding and decoding, so old and new formats coexist:
//!
//! - [`record::v1`]: the current format.
//!
//! A reader accepts any minor of a major it has a module for. So a minor bump
//! may only make changes old readers tolerate: new kinds, which they report
//! as [`DecodeError::Unknown`] and skip. Anything else is a major bump, and
//! readers without that major's module refuse the file.

pub mod layout;
pub mod reader;
pub mod record;
pub mod writer;

pub use {
  reader::{Entry, ReadError, Reader},
  record::{DecodeError, Record, Version},
  writer::Writer,
};
