//! The journal files a host has of a service's channel, for `sync` to
//! compare against its own: `GET /<service>/<api>/channels/<name>/manifest`.
//!
//! ```text
//! journal-manifest <variant>.<major>.<minor><RS><LF>
//! <prefix>live/<YYYY-MM-DD>/<instance>.journal <size in bytes><RS><LF>
//! <prefix>archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst <size> <sha-256><RS><LF>
//! ```
//!
//! One entry per file after the header, each, the header too, ending at RS
//! LF as the journal's records do, versioned like the journal's (see
//! [`Header`]): the service's variant, and its version, optionally followed
//! by informational fields. Then each of the channel's journal roots (see
//! [`Service::channel_roots`]) in turn: its live files, then its archives,
//! as their monthly manifests describe them. Paths are relative to the
//! channel's directory, `/`-separated, each starting with its root's
//! prefix: a dgg channel's journal has none and its backfill journal
//! `backfill/`; a Kick channel's has none. They're checked with
//! [`valid_path`]. (Older readers skip the entries with paths they don't
//! accept.)
//!
//! While the major is 0 (pre-release) any minor may break, so readers accept
//! only the exact variant and version they write. From 1.0 on, readers will
//! ignore entries they don't recognise, so a minor bump may add kinds of
//! entry; anything else will be a major bump.
//!
//! Each service's manifest is versioned on its own, so one changing doesn't
//! touch another's, and a new service's is just another manifest: a peer
//! without it answers `404`, and an older one never asks for it.
//!
//! Before manifests were by channel, each was a whole service's, at `GET
//! /<service>/<api>/manifest`, its paths relative to the service's
//! directory.
//!
//! Versions of dgg's:
//!
//! - `0.0`: `journal-manifest 0.0`, naming no variant.
//! - `0.1`: names the variant; entries end at RS LF, not LF. A live file may
//!   be numbered, `<instance>.<n>.journal`, as its instance's id allows.
//! - `0.2`: the header ends at RS LF too.
//!
//! Versions of Kick's:
//!
//! - `0.0`: the first, a whole service's: paths start with `<slug>/`.
//! - `0.1`: a channel's: paths are relative to it.

use {
  crate::service::{Root, Service},
  ::logger_journal::{
    archive::{self, valid_day},
    layout::{ARCHIVE_SUFFIX, EXTENSION},
    record::{Format, Header, Variant, Version, split, v0::valid_instance},
  },
  ::std::{fs, io, path::Path},
};

/// The manifest's format.
pub const FORMAT: Format = Format {
  magic: "journal-manifest",
};

/// The version of dgg's manifest this crate writes and reads.
pub const VERSION: Version = Version { major: 0, minor: 2 };

/// The version of Kick's manifest this crate writes and reads.
pub const KICK_VERSION: Version = Version { major: 0, minor: 1 };

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
  /// Relative to the channel's directory, `/`-separated.
  pub path: String,
  pub size: u64,
  /// For archives: the SHA-256 of the file, lowercase hex.
  pub sha256: Option<String>,
}

/// Whether `path`, relative to a journal root, names a live file:
/// `live/<YYYY-MM-DD>/<instance>.journal`, nothing more, so it can't reach
/// outside the journal.
pub fn live_path(path: &str) -> bool {
  let mut parts = path.split('/');
  let (Some("live"), Some(day), Some(file), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return false;
  };
  let instance = file.strip_suffix(&format!(".{EXTENSION}"));
  valid_day(day) && instance.is_some_and(valid_instance)
}

/// The day `path`, relative to a journal root, archives, if it names an
/// archive: `archive/<YYYY>/<MM>/<YYYY-MM-DD>.journal.zst`, its month
/// directories matching its day.
pub fn archive_path(path: &str) -> Option<&str> {
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
  (valid_day(day) && year == &day[..4] && month == &day[5..7]).then_some(day)
}

/// Whether `path`, relative to a channel of `S`'s directory, names one of
/// its live files or archives, in a journal root it could have.
pub fn valid_path<S: Service>(path: &str) -> bool {
  S::split(path)
    .is_some_and(|(_, rest)| live_path(rest) || archive_path(rest).is_some())
}

/// The journal root, in the directory `dir` of a channel of `S`'s, holding
/// the archive at `path`, relative to it, and its day, if `path` names one.
pub fn archive_root<'a, S: Service>(
  dir: &Path,
  path: &'a str,
) -> Option<(Root, &'a str)> {
  let (prefix, rest) = S::split(path)?;
  let day = archive_path(rest)?;
  Some((Root::new::<S>(dir, prefix, ""), day))
}

fn valid_sha256(text: &str) -> bool {
  text.len() == 64
    && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Every file of `S`'s channel `name`, in the channel's directory `dir`,
/// with its size: each journal root's live files, then its archives.
pub fn list<S: Service>(dir: &Path, name: &str) -> io::Result<Vec<File>> {
  let mut files = Vec::new();
  for root in S::channel_roots(dir, name) {
    for path in root.live_files()? {
      let relative = root.path(&path);
      if valid_path::<S>(&relative) {
        files.push(File {
          path: relative,
          size: fs::metadata(&path)?.len(),
          sha256: None,
        });
      }
    }
    for archive in archive::all(&root.dir)? {
      let day = &archive.day;
      files.push(File {
        path: format!(
          "{}archive/{}/{}/{day}{ARCHIVE_SUFFIX}",
          root.prefix,
          &day[..4],
          &day[5..7]
        ),
        size: archive.compressed,
        sha256: Some(archive.sha256),
      });
    }
  }
  Ok(files)
}

/// A manifest of `files`, `S`'s.
pub fn encode<S: Service>(files: &[File]) -> String {
  let mut text = Header::new(S::VARIANT, version(S::VARIANT)).text_as(FORMAT);
  for file in files {
    text += &format!("{} {}", file.path, file.size);
    if let Some(sha256) = &file.sha256 {
      text += &format!(" {sha256}");
    }
    text += "\u{1e}\n";
  }
  text
}

/// Parses a manifest of `S`'s, or `None` if it isn't one this crate can
/// read.
pub fn decode<S: Service>(text: &str) -> Option<Vec<File>> {
  let (first, body) = text.split_once('\n')?;
  let header = Header::parse_as(FORMAT, first.as_bytes())?;
  if header.variant != S::VARIANT || header.version != version(S::VARIANT) {
    return None;
  }
  let rest = |path| S::split(path).map(|(_, rest)| rest);
  let live = |path| rest(path).is_some_and(live_path);
  let archived = |path| rest(path).and_then(archive_path).is_some();
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
