//! The journal files a host has, for `sync` to compare against its own.
//!
//! ```text
//! journal-manifest dgg.0.2<RS><LF>
//! live/<YYYY-MM-DD>/<instance>.journal <size in bytes><RS><LF>
//! backfill/live/<YYYY-MM-DD>/<instance>.journal <size in bytes><RS><LF>
//! archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst <size> <sha-256><RS><LF>
//! backfill/archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst <size> <sha-256><RS><LF>
//! ```
//!
//! One entry per file after the header, each, the header too, ending at RS
//! LF as the journal's records do, versioned like the journal's (see
//! [`Header`]): the variant of the journal root it lists, and its version,
//! optionally followed by informational fields. Then the live files, then
//! the backfill journal's, then the archives of both,
//! as their monthly manifests describe them. Paths are relative to the
//! journal root, `/`-separated, and checked with [`valid_live_path`] and
//! [`archive_path`]. (Older readers skip the entries with paths they don't
//! accept.)
//!
//! While the major is 0 (pre-release) any minor may break, so readers accept
//! only the exact variant and version they write. From 1.0 on, readers will
//! ignore entries they don't recognise, so a minor bump may add kinds of
//! entry; anything else will be a major bump.
//!
//! Versions of dgg's:
//!
//! - `0.0`: `journal-manifest 0.0`, naming no variant.
//! - `0.1`: names the variant; entries end at RS LF, not LF. A live file may
//!   be numbered, `<instance>.<n>.journal`, as its instance's id allows.
//! - `0.2`: the header ends at RS LF too.
//!
//! Kick's journals have a manifest of their own, `journal-manifest
//! kick.0.0`, so dgg's stays as every host's `sync` reads it. It lists every
//! channel's journal root, each file's path starting `kick/<slug>/`: its
//! live files, then its archives. There's no backfill for Kick.
//!
//! ```text
//! journal-manifest kick.0.0<RS><LF>
//! kick/<slug>/live/<YYYY-MM-DD>/<instance>.journal <size in bytes><RS><LF>
//! kick/<slug>/archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst <size> <sha-256><RS><LF>
//! ```
//!
//! Versions of Kick's:
//!
//! - `0.0`: the first.

use {
  crate::kick::channels::{self, valid_slug},
  ::logger_journal::{
    archive::{self, valid_day},
    layout::{
      ARCHIVE_SUFFIX, EXTENSION, backfill_files, backfill_root, channel_root,
      kick_root, live_files,
    },
    record::{Format, Header, Variant, Version, split, v0::valid_instance},
  },
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

/// The manifest's format.
pub const FORMAT: Format = Format {
  magic: "journal-manifest",
};

/// The version of dgg's manifest this crate writes and reads.
pub const VERSION: Version = Version { major: 0, minor: 2 };

/// The version of Kick's manifest this crate writes and reads.
pub const KICK_VERSION: Version = Version { major: 0, minor: 0 };

/// The version of `variant`'s manifest this crate writes and reads.
pub fn version(variant: Variant) -> Version {
  match variant {
    Variant::Dgg => VERSION,
    Variant::Kick => KICK_VERSION,
  }
}

/// One file and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
  /// Relative to the journal root, `/`-separated.
  pub path: String,
  pub size: u64,
  /// For archives: the SHA-256 of the file, lowercase hex.
  pub sha256: Option<String>,
}

/// Whether `path` names a live journal file: `live/<YYYY-MM-DD>/<instance>
/// .journal`, or the same under `backfill/`, nothing more, so it can't reach
/// outside the journal.
pub fn valid_live_path(path: &str) -> bool {
  let path = path.strip_prefix("backfill/").unwrap_or(path);
  let mut parts = path.split('/');
  let (Some("live"), Some(day), Some(file), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return false;
  };
  let instance = file.strip_suffix(&format!(".{EXTENSION}"));
  valid_day(day) && instance.is_some_and(valid_instance)
}

/// The day `path` archives, and whether it's the backfill journal's, if it
/// names an archive: `archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst`, its
/// month directories matching its day, or the same under `backfill/`.
pub fn archive_path(path: &str) -> Option<(&str, bool)> {
  let (path, backfill) = match path.strip_prefix("backfill/") {
    Some(path) => (path, true),
    None => (path, false),
  };
  let mut parts = path.split('/');
  let (Some("archive"), Some(year), Some(month), Some(file), None) = (
    parts.next(),
    parts.next(),
    parts.next(),
    parts.next(),
    parts.next(),
  ) else {
    return None;
  };
  let day = file.strip_suffix(ARCHIVE_SUFFIX)?;
  (valid_day(day) && year == &day[..4] && month == &day[5..7])
    .then_some((day, backfill))
}

/// The channel and the rest of `path`, if it's under a Kick channel's
/// journal root: `kick/<slug>/<rest>`.
pub fn kick_path(path: &str) -> Option<(&str, &str)> {
  let (slug, rest) = path.strip_prefix("kick/")?.split_once('/')?;
  valid_slug(slug).then_some((slug, rest))
}

/// Whether `path` names a Kick channel's live file or archive:
/// `kick/<slug>/live/...` or `kick/<slug>/archive/...`, as for dgg's but
/// with no backfill.
pub fn valid_kick_path(path: &str) -> bool {
  kick_path(path).is_some_and(|(_, rest)| {
    !rest.starts_with("backfill/")
      && (valid_live_path(rest) || archive_path(rest).is_some())
  })
}

/// The journal root, under `journal_root`, holding the archive at `path`,
/// and its day, if `path` names one: dgg's, the backfill journal's or a Kick
/// channel's.
pub fn archive_root<'a>(
  journal_root: &Path,
  path: &'a str,
) -> Option<(PathBuf, &'a str)> {
  if let Some((slug, rest)) = kick_path(path) {
    let (day, backfill) = archive_path(rest)?;
    let root = channel_root(&kick_root(journal_root), slug);
    return (!backfill).then_some((root, day));
  }
  let (day, backfill) = archive_path(path)?;
  let root = match backfill {
    true => backfill_root(journal_root),
    false => journal_root.to_path_buf(),
  };
  Some((root, day))
}

fn valid_sha256(text: &str) -> bool {
  text.len() == 64
    && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Every live file under `journal_root`, then every backfill one, then the
/// archives of both, with their sizes.
pub fn list(journal_root: &Path) -> io::Result<Vec<File>> {
  let mut files = Vec::new();
  let paths = live_files(journal_root)?
    .into_iter()
    .chain(backfill_files(journal_root)?);
  for path in paths {
    let relative = path.strip_prefix(journal_root).unwrap_or(&path);
    let relative = relative.to_string_lossy().replace('\\', "/");
    if valid_live_path(&relative) {
      let size = fs::metadata(&path)?.len();
      files.push(File {
        path: relative,
        size,
        sha256: None,
      });
    }
  }
  let backfill = backfill_root(journal_root);
  for (root, prefix) in [(journal_root, ""), (backfill.as_path(), "backfill/")]
  {
    archives(root, prefix, &mut files)?;
  }
  Ok(files)
}

/// Every Kick channel's live files then archives, channel by channel, with
/// their sizes, their paths under `journal_root` (`kick/<slug>/...`).
pub fn list_kick(journal_root: &Path) -> io::Result<Vec<File>> {
  let mut files = Vec::new();
  for slug in channels::journaled(journal_root)? {
    let root = channel_root(&kick_root(journal_root), &slug);
    let prefix = format!("kick/{slug}/");
    for path in live_files(&root)? {
      let relative = path.strip_prefix(&root).unwrap_or(&path);
      let relative = relative.to_string_lossy().replace('\\', "/");
      if valid_live_path(&relative) {
        files.push(File {
          path: prefix.clone() + &relative,
          size: fs::metadata(&path)?.len(),
          sha256: None,
        });
      }
    }
    archives(&root, &prefix, &mut files)?;
  }
  Ok(files)
}

/// Adds journal `root`'s archives to `files`, their paths after `prefix`.
fn archives(
  root: &Path,
  prefix: &str,
  files: &mut Vec<File>,
) -> io::Result<()> {
  for archive in archive::all(root)? {
    let day = &archive.day;
    files.push(File {
      path: format!(
        "{prefix}archive/{}/{}/{day}{ARCHIVE_SUFFIX}",
        &day[..4],
        &day[5..7]
      ),
      size: archive.compressed,
      sha256: Some(archive.sha256),
    });
  }
  Ok(())
}

/// A manifest of `files`, of `variant`'s journals.
pub fn encode(variant: Variant, files: &[File]) -> String {
  let mut text = Header::new(variant, version(variant)).text_as(FORMAT);
  for file in files {
    text += &format!("{} {}", file.path, file.size);
    if let Some(sha256) = &file.sha256 {
      text += &format!(" {sha256}");
    }
    text += "\u{1e}\n";
  }
  text
}

/// Parses a manifest of `variant`'s journals, or `None` if it isn't one
/// this crate can read.
pub fn decode(variant: Variant, text: &str) -> Option<Vec<File>> {
  let (first, body) = text.split_once('\n')?;
  let header = Header::parse_as(FORMAT, first.as_bytes())?;
  if header.variant != variant || header.version != version(variant) {
    return None;
  }
  let live = |path: &str| match variant {
    Variant::Dgg => valid_live_path(path),
    Variant::Kick => valid_kick_path(path) && kick_archive(path).is_none(),
  };
  let archived = |path: &str| match variant {
    Variant::Dgg => archive_path(path).is_some(),
    Variant::Kick => kick_archive(path).is_some(),
  };
  let (entries, tail) = split(body.as_bytes());
  if !tail.is_empty() {
    return None;
  }
  let mut files = Vec::new();
  for entry in entries {
    let line = ::std::str::from_utf8(entry).ok()?;
    let fields: Vec<_> = line.split(' ').collect();
    let (path, size, sha256) = match fields[..] {
      [path, size] if live(path) => (path, size, None),
      [path, size, sha256] if archived(path) => {
        if !valid_sha256(sha256) {
          return None;
        }
        (path, size, Some(sha256.to_owned()))
      }
      _ => continue,
    };
    if size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
      return None;
    }
    files.push(File {
      path: path.into(),
      size: size.parse().ok()?,
      sha256,
    });
  }
  Some(files)
}

/// The day a Kick channel's archive at `path` holds, if it names one.
fn kick_archive(path: &str) -> Option<&str> {
  let (_, rest) = kick_path(path)?;
  match archive_path(rest)? {
    (day, false) => Some(day),
    (_, true) => None,
  }
}
