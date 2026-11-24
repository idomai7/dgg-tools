//! `control/synced-archives`, under each journal root: the archives `sync`
//! took from peers, by day and SHA-256, so none is taken twice.
//!
//! ```text
//! synced-archives dgg.0.2<RS><LF>
//! <YYYY-MM-DD> <sha-256><RS><LF>
//! ```
//!
//! A Kick channel's journal root has one too, the same but `synced-archives
//! kick.0.0`.
//!
//! The header, versioned like the journal's (see [`Header`]), and each
//! entry end at RS LF, as the journal's records do.
//! Entries this version doesn't recognise are skipped.
//!
//! Versions of dgg's:
//!
//! - `0.0`: no header; each entry a line, `<YYYY-MM-DD> <sha-256>`.
//! - `0.1`: the header, ending at LF; entries end at RS LF.
//! - `0.2`: the header ends at RS LF too.
//!
//! Only 0.2 is read: every host's file was migrated to it.
//!
//! Versions of Kick's:
//!
//! - `0.0`: the first.

use {
  ::common::{atomic_file, fs::MissingOk},
  ::logger_journal::{
    Format, Header, Variant, Version, archive::valid_day, record::split,
  },
  ::std::{fs, io, path::Path},
};

/// Where it is, under a journal root.
pub const PATH: &str = "control/synced-archives";

/// Its format.
pub const FORMAT: Format = Format {
  magic: "synced-archives",
};

/// The version of dgg's this crate writes and reads.
pub const VERSION: Version = Version { major: 0, minor: 2 };

/// The version of Kick's this crate writes and reads.
pub const KICK_VERSION: Version = Version { major: 0, minor: 0 };

/// The version of `variant`'s this crate writes and reads.
pub fn version(variant: Variant) -> Version {
  match variant {
    Variant::Dgg => VERSION,
    Variant::Kick => KICK_VERSION,
  }
}

/// One archive taken: its day and SHA-256.
pub type Taken = (String, String);

/// The file's contents, for a journal root of `variant`'s, in this crate's
/// version.
pub fn encode(variant: Variant, taken: &[Taken]) -> String {
  let mut text = Header::new(variant, version(variant)).text_as(FORMAT);
  for (day, sha256) in taken {
    text += &format!("{day} {sha256}\u{1e}\n");
  }
  text
}

/// Parses the file's contents, for a journal root of `variant`'s: what was
/// taken. `None` if they aren't readable.
pub fn decode(variant: Variant, text: &str) -> Option<Vec<Taken>> {
  let entry = |entry: &str| {
    let (day, sha256) = entry.split_once(' ')?;
    (valid_day(day) && valid_sha256(sha256))
      .then(|| (day.into(), sha256.into()))
  };
  let (first, body) = text.split_once('\n')?;
  let header = Header::parse_as(FORMAT, first.as_bytes())?;
  if header.variant != variant || header.version != version(variant) {
    return None;
  }
  let (entries, tail) = split(body.as_bytes());
  if !tail.is_empty() {
    return None;
  }
  let taken = entries
    .into_iter()
    .filter_map(|entry| ::std::str::from_utf8(entry).ok())
    .filter_map(entry)
    .collect();
  Some(taken)
}

/// What journal `root`'s file lists, `root` being of `variant`'s; nothing
/// if there's none.
pub fn read(root: &Path, variant: Variant) -> io::Result<Vec<Taken>> {
  let path = root.join(PATH);
  let Some(text) = fs::read_to_string(&path).missing_ok()? else {
    return Ok(Vec::new());
  };
  decode(variant, &text).ok_or_else(|| {
    let why = format!("{}: not a readable synced-archives", path.display());
    io::Error::new(io::ErrorKind::InvalidData, why)
  })
}

/// Adds the archive of `day` with `sha256` to journal `root`'s file, `root`
/// being of `variant`'s, rewriting it whole, atomically, in this crate's
/// version.
pub fn add(
  root: &Path,
  variant: Variant,
  day: &str,
  sha256: &str,
) -> io::Result<()> {
  let mut taken = read(root, variant)?;
  taken.push((day.into(), sha256.into()));
  atomic_file::replace(&root.join(PATH), encode(variant, &taken))
}

fn valid_sha256(text: &str) -> bool {
  text.len() == 64
    && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
