//! `control/synced-archives`, under each journal root: the archives `sync`
//! took from peers, by day and SHA-256, so none is taken twice.
//!
//! ```text
//! synced-archives dgg.0.2<RS><LF>
//! <YYYY-MM-DD> <sha-256><RS><LF>
//! ```
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

use {
  ::common::{atomic_file, fs::MissingOk},
  ::logger_journal::{
    Format, Header, Variant, Version, archive::valid_day, record::Framing,
  },
  ::std::{fs, io, path::Path},
};

/// Where it is, under a journal root.
pub const PATH: &str = "control/synced-archives";

/// Its format.
pub const FORMAT: Format = Format {
  magic: "synced-archives",
  last_unnamed: Version { major: 0, minor: 0 },
  rs_header: &[(Variant::Dgg, Version { major: 0, minor: 2 })],
};

/// The version this crate writes and reads.
pub const VERSION: Version = Version { major: 0, minor: 2 };

/// One archive taken: its day and SHA-256.
pub type Taken = (String, String);

/// The file's contents, in this crate's version.
pub fn encode(taken: &[Taken]) -> String {
  let mut text = Header::new(Variant::Dgg, VERSION).text_as(FORMAT);
  for (day, sha256) in taken {
    text += &format!("{day} {sha256}\u{1e}\n");
  }
  text
}

/// Parses the file's contents: what was taken. `None` if they aren't
/// readable.
pub fn decode(text: &str) -> Option<Vec<Taken>> {
  let entry = |entry: &str| {
    let (day, sha256) = entry.split_once(' ')?;
    (valid_day(day) && valid_sha256(sha256))
      .then(|| (day.into(), sha256.into()))
  };
  let (first, body) = text.split_once('\n')?;
  let header = Header::parse_as(FORMAT, first.as_bytes())?;
  if header.variant != Variant::Dgg || header.version != VERSION {
    return None;
  }
  let (entries, tail) = Framing::RsLf.split(body.as_bytes());
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

/// What journal `root`'s file lists; nothing if there's none.
pub fn read(root: &Path) -> io::Result<Vec<Taken>> {
  let path = root.join(PATH);
  let Some(text) = fs::read_to_string(&path).missing_ok()? else {
    return Ok(Vec::new());
  };
  decode(&text).ok_or_else(|| {
    let why = format!("{}: not a readable synced-archives", path.display());
    io::Error::new(io::ErrorKind::InvalidData, why)
  })
}

/// Adds the archive of `day` with `sha256` to journal `root`'s file,
/// rewriting it whole, atomically, in this crate's version.
pub fn add(root: &Path, day: &str, sha256: &str) -> io::Result<()> {
  let mut taken = read(root)?;
  taken.push((day.into(), sha256.into()));
  atomic_file::replace(&root.join(PATH), encode(&taken))
}

fn valid_sha256(text: &str) -> bool {
  text.len() == 64
    && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
