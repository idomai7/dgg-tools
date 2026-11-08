//! Sync: pull another host's live journal files into this one's.
//!
//! Journal files are only ever appended to, so a local copy is always a
//! prefix of the remote file. Each round fetches the [manifest](crate::
//! manifest) and, for every remote file longer than its local copy, just
//! the missing bytes, appended and fsynced. An interrupted round leaves a
//! shorter prefix, which the next round extends. A line still being written
//! remotely is copied as far as it goes; readers wait for its newline. A
//! remote file shorter than its local copy has been rewritten, so it's
//! reported and left alone.

use {
  crate::{capture::Peer, manifest},
  ::std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
  },
};

/// What one [`sync`] round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  /// Files the peer listed.
  pub files: u64,
  /// Files that grew here.
  pub updated: u64,
  /// Bytes appended.
  pub bytes: u64,
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
