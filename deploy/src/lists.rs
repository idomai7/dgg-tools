//! The channel lists in an install's root: dgg's, `dgg-channels.jsonc`,
//! and Kick's, `kick-channels.jsonc`. Each was a plain file before, a line
//! per channel (see `PLAIN_FILE` in [`dgg::channels`] and
//! [`kick::channels`]).
//!
//! [`write_missing`] writes a list that isn't there, converted from its
//! plain one if there's one, so nothing listed is lost; else dgg's with
//! destiny.gg alone, and Kick's with none. The plain one is left, as an
//! old install still running reads it; [`retire_plain`] renames it to
//! `<name>.old` once nothing does.

use {
  ::common::{atomic_file, fs::MissingOk},
  ::logger::{dgg, kick},
  ::std::{fs, io, path::Path},
};

/// A channel list: its file, its plain file from before, how the one's
/// text is made from the other's, and its text when there's neither.
struct List {
  file: &'static str,
  plain: &'static str,
  convert: fn(&str) -> Result<String, String>,
  new: fn() -> String,
}

const LISTS: [List; 2] = [
  List {
    file: dgg::channels::FILE,
    plain: dgg::channels::PLAIN_FILE,
    convert: dgg::channels::convert_plain,
    new: || dgg::channels::render(&[dgg::channels::Channel::destiny()]),
  },
  List {
    file: kick::channels::FILE,
    plain: kick::channels::PLAIN_FILE,
    convert: kick::channels::convert_plain,
    new: || kick::channels::render(&[]),
  },
];

/// Every list's files, its own and its plain one, which move with an
/// install's root.
pub const FILES: [&str; 4] = [
  dgg::channels::FILE,
  dgg::channels::PLAIN_FILE,
  kick::channels::FILE,
  kick::channels::PLAIN_FILE,
];

/// Writes each list in `root` that isn't there: converted from its plain
/// one, or new. What it did, a line each.
pub fn write_missing(root: &Path) -> Result<Vec<String>, String> {
  let mut did = Vec::new();
  for list in &LISTS {
    let path = root.join(list.file);
    if path.exists() {
      continue;
    }
    let plain = root.join(list.plain);
    let in_path = |path: &Path, e: String| format!("{}: {e}", path.display());
    let read = fs::read_to_string(&plain).missing_ok();
    let text = match read.map_err(|e| in_path(&plain, e.to_string()))? {
      Some(text) => {
        let text = (list.convert)(&text).map_err(|e| in_path(&plain, e))?;
        did.push(format!("{}: converted from {}", list.file, list.plain));
        text
      }
      None => {
        did.push(format!("{}: written", list.file));
        (list.new)()
      }
    };
    atomic_file::replace(&path, text)
      .map_err(|e| in_path(&path, e.to_string()))?;
  }
  Ok(did)
}

/// Renames each plain list in `root` beside its list to `<name>.old`,
/// replacing any there. What it did, a line each.
pub fn retire_plain(root: &Path) -> io::Result<Vec<String>> {
  let mut did = Vec::new();
  for list in &LISTS {
    let plain = root.join(list.plain);
    if !plain.exists() || !root.join(list.file).exists() {
      continue;
    }
    let old = format!("{}.old", list.plain);
    atomic_file::rename(&plain, &root.join(&old))?;
    did.push(format!(
      "{}: renamed to {old}, as {} has it",
      list.plain, list.file
    ));
  }
  Ok(did)
}
