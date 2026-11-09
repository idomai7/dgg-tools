//! Sync: pull another host's live journal files, and its backfill files,
//! into this one's.
//!
//! Journal files are only ever appended to, so a local copy is always a
//! prefix of the remote file. Each round fetches the [manifest](crate::
//! manifest) and, for every remote file longer than its local copy, just
//! the missing bytes, appended and fsynced. An interrupted round leaves a
//! shorter prefix, which the next round extends. A line still being written
//! remotely is copied as far as it goes; readers wait for its newline. A
//! remote file shorter than its local copy has been rewritten, so it's
//! reported and left alone.
//!
//! Archives are taken whole, each checked against the SHA-256 and size the
//! manifest gives, and never overwrite anything here. One for a day with
//! nothing here (no archive, no live files) is installed as it is, so both
//! hosts hold the same bytes. Otherwise it's decompressed into a live file,
//! `live/<day>/archive-<sha-256 prefix>.journal`, which the next compaction
//! merges with the rest of the day. Either way it's recorded in
//! [`TAKEN`], and not taken again.

use {
  crate::{
    capture::Peer,
    manifest::{self, archive_path},
  },
  ::logger_journal::{
    archive::{self, Archive},
    layout::{EXTENSION, archive_file, backfill_root, live_day_dir},
    record::Version,
  },
  ::sha2::{Digest, Sha256},
  ::std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
  },
};

/// The file, under each journal root (the journal's and the backfill
/// journal's), listing the archives taken from peers: `<day> <sha-256>`
/// lines.
pub const TAKEN: &str = "control/synced-archives";

/// What one [`sync`] round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  /// Files the peer listed.
  pub files: u64,
  /// Files that grew here.
  pub updated: u64,
  /// Bytes appended, or downloaded as archives.
  pub bytes: u64,
  /// Archives installed as they are, there being nothing here for their
  /// day.
  pub installed: u64,
  /// Archives written out as live files, for compaction to merge with what
  /// is here for their day.
  pub merging: u64,
  /// Files shorter on the peer than here, which aren't touched.
  pub mismatched: Vec<String>,
}

fn client() -> ::reqwest::Client {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  ::reqwest::Client::builder()
    .timeout(::std::time::Duration::from_secs(60))
    .build()
    .expect("http client")
}

/// Pulls everything `peer` has that `journal_root` doesn't.
pub async fn sync(peer: &Peer, journal_root: &Path) -> Result<Stats, String> {
  let client = client();
  let base = peer.url.trim_end_matches('/');
  let get = |path: String| client.get(format!("{base}/{path}"));
  let response = get("manifest".into())
    .bearer_auth(&peer.token)
    .send()
    .await
    .map_err(|e| e.to_string())?;
  if !response.status().is_success() {
    return Err(format!("manifest: HTTP {}", response.status()));
  }
  let text = response.text().await.map_err(|e| e.to_string())?;
  let files =
    manifest::decode(&text).ok_or("manifest isn't a readable manifest")?;

  let mut stats = Stats {
    files: files.len() as u64,
    ..Stats::default()
  };
  for file in files {
    if let Some(sha256) = &file.sha256 {
      let Some((day, backfill)) = archive_path(&file.path) else {
        continue;
      };
      let root = match backfill {
        true => backfill_root(journal_root),
        false => journal_root.to_path_buf(),
      };
      let fail = |e: &dyn ::std::fmt::Display| format!("{}: {e}", file.path);
      if !wanted(&root, day, sha256).map_err(|e| fail(&e))? {
        continue;
      }
      let response = get(format!("files/{}", file.path))
        .bearer_auth(&peer.token)
        .send()
        .await
        .map_err(|e| fail(&e))?;
      if response.status() != ::reqwest::StatusCode::OK {
        return Err(fail(&format_args!("HTTP {}", response.status())));
      }
      let bytes = response.bytes().await.map_err(|e| fail(&e))?;
      let installed =
        take(&root, day, sha256, file.size, &bytes).map_err(|e| fail(&e))?;
      if installed {
        stats.installed += 1;
      } else {
        stats.merging += 1;
      }
      stats.bytes += bytes.len() as u64;
      continue;
    }
    let local = journal_root.join(&file.path);
    let have = match fs::metadata(&local) {
      Ok(metadata) => metadata.len(),
      Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
      Err(e) => return Err(format!("{}: {e}", file.path)),
    };
    if file.size < have {
      stats.mismatched.push(file.path);
      continue;
    }
    if file.size == have {
      continue;
    }
    let response = get(format!("files/{}", file.path))
      .bearer_auth(&peer.token)
      .header("Range", format!("bytes={have}-"))
      .send()
      .await
      .map_err(|e| format!("{}: {e}", file.path))?;
    if response.status() != ::reqwest::StatusCode::PARTIAL_CONTENT {
      return Err(format!("{}: HTTP {}", file.path, response.status()));
    }
    let bytes = response
      .bytes()
      .await
      .map_err(|e| format!("{}: {e}", file.path))?;
    append(&local, &bytes).map_err(|e| format!("{}: {e}", file.path))?;
    stats.updated += 1;
    stats.bytes += bytes.len() as u64;
  }
  Ok(stats)
}

/// Appends `bytes` to `path`, creating it and its directory if needed, and
/// fsyncs it.
fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
  if let Some(directory) = path.parent() {
    fs::create_dir_all(directory)?;
  }
  let mut file = OpenOptions::new().create(true).append(true).open(path)?;
  file.write_all(bytes)?;
  file.sync_data()
}

fn invalid(why: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, why)
}

/// The archives listed in `root`'s [`TAKEN`], as `(day, sha-256)`.
fn taken(root: &Path) -> io::Result<Vec<(String, String)>> {
  let text = match fs::read_to_string(root.join(TAKEN)) {
    Ok(text) => text,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(e),
  };
  Ok(
    text
      .lines()
      .filter_map(|line| line.split_once(' '))
      .map(|(day, sha256)| (day.into(), sha256.into()))
      .collect(),
  )
}

/// Whether the archive of `day` with `sha256` is new to journal `root`: it
/// isn't the archive here, and wasn't taken before.
fn wanted(root: &Path, day: &str, sha256: &str) -> io::Result<bool> {
  let here = archive::read_month(root, day)?;
  if here
    .iter()
    .any(|archive| archive.day == day && archive.sha256 == sha256)
  {
    return Ok(false);
  }
  let taken = taken(root)?;
  Ok(!taken.iter().any(|(d, s)| d == day && s == sha256))
}

/// Whether journal `root` has live files for `day`.
fn has_live(root: &Path, day: &str) -> io::Result<bool> {
  let entries = match fs::read_dir(live_day_dir(root, day)) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
    Err(e) => return Err(e),
  };
  for entry in entries {
    if entry?
      .path()
      .extension()
      .is_some_and(|extension| extension == EXTENSION)
    {
      return Ok(true);
    }
  }
  Ok(false)
}

/// Takes `compressed`, the peer's archive of `day`, into journal `root`
/// (see the [module docs](self)), once it matches `sha256` and `size`;
/// whether it was installed as it is.
fn take(
  root: &Path,
  day: &str,
  sha256: &str,
  size: u64,
  compressed: &[u8],
) -> io::Result<bool> {
  let digest: String = Sha256::digest(compressed)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
  if compressed.len() as u64 != size || digest != sha256 {
    return Err(invalid("archive doesn't match the manifest"));
  }
  let journal = ::zstd::decode_all(compressed)?;
  let header = journal.split(|&b| b == b'\n').next().unwrap_or_default();
  if !Version::parse_header(header).is_some_and(|version| version.readable()) {
    return Err(invalid("archive isn't a readable journal"));
  }
  let archived = archive::read_month(root, day)?
    .iter()
    .any(|archive| archive.day == day)
    || archive_file(root, day).exists();
  let installed = !archived && !has_live(root, day)?;
  if installed {
    write_new(&archive_file(root, day), compressed)?;
    // Every line ends in a newline, the header's too.
    let lines = journal.iter().filter(|&&b| b == b'\n').count() as u64 - 1;
    archive::record(
      root,
      &Archive {
        day: day.into(),
        compressed: size,
        bytes: journal.len() as u64,
        lines,
        sha256: sha256.into(),
      },
    )?;
  } else {
    let name = format!("archive-{}.{EXTENSION}", &sha256[..16]);
    write_new(&live_day_dir(root, day).join(name), &journal)?;
  }
  let path = root.join(TAKEN);
  fs::create_dir_all(path.parent().unwrap())?;
  let mut file = OpenOptions::new().create(true).append(true).open(path)?;
  file.write_all(format!("{day} {sha256}\n").as_bytes())?;
  file.sync_data()?;
  Ok(installed)
}

/// Writes `bytes` to `path` through a fsynced temporary file renamed into
/// place.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
  fs::create_dir_all(path.parent().unwrap())?;
  let mut temporary = path.as_os_str().to_owned();
  temporary.push(".tmp");
  let temporary = PathBuf::from(temporary);
  let mut file = File::create(&temporary)?;
  file.write_all(bytes)?;
  file.sync_all()?;
  drop(file);
  fs::rename(temporary, path)
}
