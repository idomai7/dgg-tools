//! Sync: pull another host's journals, every service's, into this one's.
//!
//! Each service is pulled in turn, by the same rules: its [manifest](crate::
//! manifest) from `GET /<service>/<api>/manifest`, its files from `GET
//! /<service>/<api>/files/<path>`, each path relative to its directory
//! there and here. A peer without a service, or without its API's version
//! (see [`Service::API`]), answers `404`: it has none of its journals.
//!
//! Journal files are only ever appended to, so a local copy is always a
//! prefix of the remote file. Each round fetches the manifest and, for
//! every remote file longer than its local copy, just the missing bytes,
//! appended and fsynced. An interrupted round leaves a
//! shorter prefix, which the next round extends. A record still being
//! written remotely is copied as far as it goes; readers wait for its end. A
//! remote file shorter than its local copy has been rewritten, so it's
//! reported and left alone.
//!
//! Archives are taken whole, each checked against the SHA-256 and size the
//! manifest gives, and never overwrite anything here. One for a day with
//! nothing here (no archive, no live files) is installed as it is, so both
//! hosts hold the same bytes. Otherwise it's decompressed into a live file,
//! `live/<day>/archive-<sha-256 prefix>.journal`, which the next compaction
//! merges with the rest of the day. Either way it's recorded in
//! [`TAKEN`] (see [`taken`](crate::taken)), in its journal root, and not
//! taken again.
//!
//! The peer is trusted for what the journal says, but not for how much:
//! a manifest over [`MAX_MANIFEST`], or a file over [`MAX_FILE`] (listed,
//! sent, or decompressed), fails the round. Downloads go to disk as they
//! arrive, so memory use stays small whatever the peer sends.

use {
  crate::{
    capture::Peer,
    dgg::Dgg,
    kick::Kick,
    manifest::{self, archive_root},
    service::{Root, Service, locate, routes},
  },
  ::common::{atomic_file, fs::MissingOk},
  ::logger_journal::{
    archive::{self, Archive},
    layout::{EXTENSION, archive_file, live_day_dir},
    record::{Header, RS, Variant},
  },
  ::sha2::{Digest, Sha256},
  ::std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    time::Duration,
  },
};

/// The file, under each journal root, listing the archives taken from
/// peers (see [`taken`](crate::taken)).
pub const TAKEN: &str = crate::taken::PATH;

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
/// Trusts `peer`'s CA alone, if it has one.
fn client(peer: &Peer) -> Result<::reqwest::Client, String> {
  peer.client(
    ::reqwest::Client::builder()
      .redirect(::reqwest::redirect::Policy::none())
      .connect_timeout(Duration::from_secs(20))
      .read_timeout(Duration::from_secs(60)),
  )
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

/// Pulls everything `peer` has that the journal directory `journal`
/// doesn't: dgg's journals, then Kick's.
pub async fn sync(peer: &Peer, journal: &Path) -> Result<Stats, String> {
  let client = client(peer)?;
  let mut stats = Stats::default();
  pull::<Dgg>(&client, peer, journal, &mut stats).await?;
  pull::<Kick>(&client, peer, journal, &mut stats).await?;
  Ok(stats)
}

/// `peer`'s manifest of `S`'s journals: none if it doesn't have `S`.
async fn fetch_manifest<S: Service>(
  client: &::reqwest::Client,
  peer: &Peer,
) -> Result<Vec<manifest::File>, String> {
  let path = format!("{}/manifest", routes::<S>());
  let base = peer.url.trim_end_matches('/');
  let response = client
    .get(format!("{base}{path}"))
    .bearer_auth(&peer.token)
    .send()
    .await
    .map_err(|e| e.to_string())?;
  if response.status() == ::reqwest::StatusCode::NOT_FOUND {
    return Ok(Vec::new());
  }
  if !response.status().is_success() {
    return Err(format!("{path}: HTTP {}", response.status()));
  }
  let mut text = Vec::new();
  let more = body(response, MAX_MANIFEST, |chunk| {
    text.extend_from_slice(chunk);
    Ok(())
  })
  .await
  .map_err(|e| format!("{path}: {e}"))?;
  if more {
    return Err(format!("{path}: over {MAX_MANIFEST} bytes"));
  }
  let files = String::from_utf8(text)
    .ok()
    .and_then(|text| manifest::decode::<S>(&text))
    .ok_or(format!("{path} isn't a readable manifest"))?;
  if let Some(file) = files.iter().find(|file| file.size > MAX_FILE) {
    return Err(format!("{}: listed as over {MAX_FILE} bytes", file.path));
  }
  Ok(files)
}

/// Pulls each file in `peer`'s manifest of `S`'s journals that the
/// journal directory `journal` doesn't have, adding what was done to
/// `stats`.
async fn pull<S: Service>(
  client: &::reqwest::Client,
  peer: &Peer,
  journal: &Path,
  stats: &mut Stats,
) -> Result<(), String> {
  let files = fetch_manifest::<S>(client, peer).await?;
  stats.files += files.len() as u64;
  let dir = S::dir(journal);
  let variant = S::VARIANT;
  let base = peer.url.trim_end_matches('/');
  let at = routes::<S>();
  let get = |path: &str| client.get(format!("{base}{at}/files/{path}"));
  for file in files {
    if let Some(sha256) = &file.sha256 {
      let Some((root, day)) = archive_root::<S>(&dir, &file.path) else {
        continue;
      };
      let fail = |e: &dyn ::std::fmt::Display| format!("{}: {e}", file.path);
      if !wanted(&root, variant, day, sha256).map_err(|e| fail(&e))? {
        continue;
      }
      let response = get(&file.path)
        .bearer_auth(&peer.token)
        .send()
        .await
        .map_err(|e| fail(&e))?;
      if response.status() != ::reqwest::StatusCode::OK {
        return Err(fail(&format_args!("HTTP {}", response.status())));
      }
      let download = root.dir.join(format!("control/sync-{day}.download"));
      let taken = download_archive(response, &download, file.size, sha256)
        .await
        .and_then(|()| {
          take(&root, variant, day, sha256, file.size, &download)
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
    let local = locate::<S>(&dir, &file.path);
    let have = fs::metadata(&local)
      .missing_ok()
      .map_err(|e| format!("{}: {e}", file.path))?
      .map_or(0, |metadata| metadata.len());
    if file.size < have {
      stats.mismatched.push(file.path);
      continue;
    }
    if file.size == have {
      continue;
    }
    let response = get(&file.path)
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
  Ok(())
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

/// Whether the archive of `day` with `sha256` is new to journal `root`, of
/// `variant`'s: it isn't the archive here, and wasn't taken before.
fn wanted(
  root: &Root,
  variant: Variant,
  day: &str,
  sha256: &str,
) -> io::Result<bool> {
  let here = archive::read_month(&root.dir, day)?;
  if here
    .iter()
    .any(|archive| archive.day == day && archive.sha256 == sha256)
  {
    return Ok(false);
  }
  let taken = crate::taken::read(&root.dir, variant)?;
  Ok(!taken.iter().any(|(d, s)| d == day && s == sha256))
}

/// Whether journal `root` has live files for `day`, in its directory or
/// its legacy one.
fn has_live(root: &Root, day: &str) -> io::Result<bool> {
  for dir in root.dirs() {
    let Some(entries) = fs::read_dir(live_day_dir(dir, day)).missing_ok()?
    else {
      continue;
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
  }
  Ok(false)
}

/// Decompresses the archive at `path` into `out`, if given; its size and
/// records (after the header, counted by their ends). Fails if it isn't a
/// readable journal of `variant`'s, or decompresses to over `limit` bytes.
pub fn unpack(
  path: &Path,
  variant: Variant,
  mut out: Option<&mut File>,
  limit: u64,
) -> io::Result<(u64, u64)> {
  let unreadable = || invalid("archive isn't a readable journal of its kind");
  let mut decoder = ::zstd::Decoder::new(File::open(path)?)?;
  let mut buffer = vec![0; 64 << 10];
  let (mut bytes, mut records) = (0_u64, 0_u64);
  let mut head = Vec::new();
  let mut header = false;
  // The byte before, for an RS LF across chunks.
  let mut last = 0;
  loop {
    let read = decoder.read(&mut buffer)?;
    if read == 0 {
      break;
    }
    let chunk = &buffer[..read];
    bytes += read as u64;
    if bytes > limit {
      return Err(invalid(&format!(
        "archive decompresses to over {limit} bytes"
      )));
    }
    if let Some(out) = out.as_deref_mut() {
      out.write_all(chunk)?;
    }
    let body = if header {
      chunk
    } else {
      head.extend_from_slice(chunk);
      let Some(end) = head.iter().position(|&b| b == b'\n') else {
        if head.len() > 1024 {
          return Err(unreadable());
        }
        continue;
      };
      Header::parse(&head[..end])
        .filter(|h| h.variant == variant && h.readable())
        .ok_or_else(unreadable)?;
      header = true;
      // What of the head follows the header, kept apart from `chunk`.
      head.drain(..=end);
      &head[..]
    };
    for &b in body {
      if b == b'\n' && last == RS {
        records += 1;
      }
      last = b;
    }
  }
  if !header {
    return Err(unreadable());
  }
  Ok((bytes, records))
}

/// Takes `download`, the peer's archive of `day`, checked against the
/// manifest's `sha256` and `size`, into journal `root`, of `variant`'s (see
/// the [module docs](self)); whether it was installed as it is.
fn take(
  root: &Root,
  variant: Variant,
  day: &str,
  sha256: &str,
  size: u64,
  download: &Path,
) -> io::Result<bool> {
  let installed = !archived(&root.dir, day)? && !has_live(root, day)?;
  let root = root.dir.as_path();
  if installed {
    let (bytes, records) = unpack(download, variant, None, MAX_FILE)?;
    let path = archive_file(root, day);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::rename(download, path)?;
    archive::record(
      root,
      variant,
      &Archive {
        day: day.into(),
        compressed: size,
        bytes,
        records,
        sha256: sha256.into(),
      },
    )?;
  } else {
    let name = format!("archive-{}.{EXTENSION}", &sha256[..16]);
    write_new(&live_day_dir(root, day).join(name), |file| {
      unpack(download, variant, Some(file), MAX_FILE).map(|_| ())
    })?;
  }
  crate::taken::add(root, variant, day, sha256)?;
  Ok(installed)
}

/// Whether journal `root` has an archive for `day`.
fn archived(root: &Path, day: &str) -> io::Result<bool> {
  Ok(
    archive::read_month(root, day)?
      .iter()
      .any(|archive| archive.day == day)
      || archive_file(root, day).exists(),
  )
}

/// Writes `path` with `fill`, [replacing](atomic_file) it whole.
fn write_new(
  path: &Path,
  fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
  atomic_file::replace_with(path, fill)
}
