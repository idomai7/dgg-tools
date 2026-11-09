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
//!
//! The peer is trusted for what the journal says, but not for how much:
//! a manifest over [`MAX_MANIFEST`], or a file over [`MAX_FILE`] (listed,
//! sent, or decompressed), fails the round. Downloads go to disk as they
//! arrive, so memory use stays small whatever the peer sends.

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
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
  },
};

/// The file, under each journal root (the journal's and the backfill
/// journal's), listing the archives taken from peers: `<day> <sha-256>`
/// lines.
pub const TAKEN: &str = "control/synced-archives";

/// The largest manifest taken from a peer. A year of files lists in well
/// under 1 MB.
pub const MAX_MANIFEST: u64 = 16 << 20;

/// The largest file taken from a peer, compressed or not. A busy day's
/// journal is a few hundred MB.
pub const MAX_FILE: u64 = 4 << 30;

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

/// A client that doesn't follow redirects (the token would go along), and
/// gives up on a peer that stops sending, however long a download takes.
fn client() -> ::reqwest::Client {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  ::reqwest::Client::builder()
    .redirect(::reqwest::redirect::Policy::none())
    .connect_timeout(Duration::from_secs(20))
    .read_timeout(Duration::from_secs(60))
    .build()
    .expect("http client")
}

/// Hands `response`'s body to `each`, a chunk at a time, up to `limit`
/// bytes; whether there was more.
async fn body(
  mut response: ::reqwest::Response,
  limit: u64,
  mut each: impl FnMut(&[u8]) -> io::Result<()>,
) -> Result<bool, String> {
  let mut taken = 0;
  while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
    let room = usize::try_from(limit - taken).unwrap_or(usize::MAX);
    each(&chunk[..chunk.len().min(room)]).map_err(|e| e.to_string())?;
    taken += chunk.len().min(room) as u64;
    if chunk.len() > room {
      return Ok(true);
    }
  }
  Ok(false)
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
  let mut text = Vec::new();
  let more = body(response, MAX_MANIFEST, |chunk| {
    text.extend_from_slice(chunk);
    Ok(())
  })
  .await
  .map_err(|e| format!("manifest: {e}"))?;
  if more {
    return Err(format!("manifest: over {MAX_MANIFEST} bytes"));
  }
  let files = String::from_utf8(text)
    .ok()
    .and_then(|text| manifest::decode(&text))
    .ok_or("manifest isn't a readable manifest")?;
  if let Some(file) = files.iter().find(|file| file.size > MAX_FILE) {
    return Err(format!("{}: listed as over {MAX_FILE} bytes", file.path));
  }

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
      let download = root.join(format!("control/sync-{day}.download"));
      let taken = download_archive(response, &download, file.size, sha256)
        .await
        .and_then(|()| {
          take(&root, day, sha256, file.size, &download)
            .map_err(|e| e.to_string())
        });
      let _ = fs::remove_file(&download);
      if taken.map_err(|e| fail(&e))? {
        stats.installed += 1;
      } else {
        stats.merging += 1;
      }
      stats.bytes += file.size;
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
    let appended = append(&local, response, file.size - have)
      .await
      .map_err(|e| format!("{}: {e}", file.path))?;
    stats.updated += 1;
    stats.bytes += appended;
  }
  Ok(stats)
}

/// Appends up to `limit` bytes of `response` to `path`, as they arrive,
/// creating it and its directory if needed, and fsyncs it; how many. (The
/// file may have grown since the manifest; the rest comes next round.)
async fn append(
  path: &Path,
  response: ::reqwest::Response,
  limit: u64,
) -> Result<u64, String> {
  if let Some(directory) = path.parent() {
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
  }
  let mut file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(path)
    .map_err(|e| e.to_string())?;
  let mut appended = 0;
  // Stopped early, what was written is still a prefix of the peer's file.
  let written = body(response, limit, |chunk| {
    appended += chunk.len() as u64;
    file.write_all(chunk)
  })
  .await;
  file.sync_data().map_err(|e| e.to_string())?;
  written.map(|_| appended)
}

/// Writes `response`, an archive, to `path`, checking it's `size` bytes
/// with `sha256`, and fsyncs it.
async fn download_archive(
  response: ::reqwest::Response,
  path: &Path,
  size: u64,
  sha256: &str,
) -> Result<(), String> {
  fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
  let mut file = File::create(path).map_err(|e| e.to_string())?;
  let mut hasher = Sha256::new();
  let mut received = 0;
  let more = body(response, size, |chunk| {
    hasher.update(chunk);
    received += chunk.len() as u64;
    file.write_all(chunk)
  })
  .await?;
  if more || received != size || hex(&hasher.finalize()) != sha256 {
    return Err("archive doesn't match the manifest".into());
  }
  file.sync_all().map_err(|e| e.to_string())
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
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

/// Decompresses the archive at `path` into `out`, if given; its size and
/// lines (after the header). Fails if it isn't a readable journal, or
/// decompresses to over `limit` bytes.
pub fn unpack(
  path: &Path,
  mut out: Option<&mut File>,
  limit: u64,
) -> io::Result<(u64, u64)> {
  let mut decoder = ::zstd::Decoder::new(File::open(path)?)?;
  let mut buffer = vec![0; 64 << 10];
  let (mut bytes, mut newlines) = (0_u64, 0_u64);
  let mut header = Vec::new();
  loop {
    let read = decoder.read(&mut buffer)?;
    if read == 0 {
      break;
    }
    let chunk = &buffer[..read];
    if newlines == 0 {
      header.extend_from_slice(chunk);
      if let Some(end) = header.iter().position(|&b| b == b'\n') {
        let readable = Version::parse_header(&header[..end])
          .is_some_and(|version| version.readable());
        if !readable {
          return Err(invalid("archive isn't a readable journal"));
        }
      } else if header.len() > 1024 {
        return Err(invalid("archive isn't a readable journal"));
      }
    }
    bytes += read as u64;
    newlines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
    if bytes > limit {
      return Err(invalid(&format!(
        "archive decompresses to over {limit} bytes"
      )));
    }
    if let Some(out) = out.as_deref_mut() {
      out.write_all(chunk)?;
    }
  }
  if newlines == 0 {
    return Err(invalid("archive isn't a readable journal"));
  }
  // Every line ends in a newline, the header's too.
  Ok((bytes, newlines - 1))
}

/// Takes `download`, the peer's archive of `day`, checked against the
/// manifest's `sha256` and `size`, into journal `root` (see the [module
/// docs](self)); whether it was installed as it is.
fn take(
  root: &Path,
  day: &str,
  sha256: &str,
  size: u64,
  download: &Path,
) -> io::Result<bool> {
  let archived = archive::read_month(root, day)?
    .iter()
    .any(|archive| archive.day == day)
    || archive_file(root, day).exists();
  let installed = !archived && !has_live(root, day)?;
  if installed {
    let (bytes, lines) = unpack(download, None, MAX_FILE)?;
    let path = archive_file(root, day);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::rename(download, path)?;
    archive::record(
      root,
      &Archive {
        day: day.into(),
        compressed: size,
        bytes,
        lines,
        sha256: sha256.into(),
      },
    )?;
  } else {
    let name = format!("archive-{}.{EXTENSION}", &sha256[..16]);
    write_new(&live_day_dir(root, day).join(name), |file| {
      unpack(download, Some(file), MAX_FILE).map(|_| ())
    })?;
  }
  let path = root.join(TAKEN);
  fs::create_dir_all(path.parent().unwrap())?;
  let mut file = OpenOptions::new().create(true).append(true).open(path)?;
  file.write_all(format!("{day} {sha256}\n").as_bytes())?;
  file.sync_data()?;
  Ok(installed)
}

/// Writes `path` with `fill`, through a fsynced temporary file renamed into
/// place; the temporary file is removed if `fill` fails.
fn write_new(
  path: &Path,
  fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
  fs::create_dir_all(path.parent().unwrap())?;
  let mut temporary = path.as_os_str().to_owned();
  temporary.push(".tmp");
  let temporary = PathBuf::from(temporary);
  let mut file = File::create(&temporary)?;
  if let Err(e) = fill(&mut file).and_then(|()| file.sync_all()) {
    drop(file);
    let _ = fs::remove_file(&temporary);
    return Err(e);
  }
  drop(file);
  fs::rename(temporary, path)
}
