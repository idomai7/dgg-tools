use {
  ::logger::{
    capture::Peer,
    compact::compact_day,
    manifest::{self, File, archive_path, valid_live_path},
    serve::{Tokens, router},
    sync::{MAX_FILE, TAKEN, sync, unpack},
  },
  ::logger_journal::{Writer, record::v0::Body},
  ::std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
  },
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("logger-sync-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

/// Serves `root` on a free port; returns its URL.
async fn serve(root: &TempRoot) -> String {
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let url = format!("http://{}", listener.local_addr().unwrap());
  let tokens = Tokens {
    read: "secret".into(),
    control: None,
  };
  let app = router(root.0.clone(), tokens);
  ::tokio::spawn(async move { ::axum::serve(listener, app).await.unwrap() });
  url
}

#[test]
fn manifest_format() {
  assert!(valid_live_path("live/2026-11-08/a-1.journal"));
  assert!(valid_live_path(
    "backfill/live/2026-11-08/backfill-a.journal"
  ));
  for bad in [
    "backfill/archive/2026/11/x.journal",
    "backfill/backfill/live/2026-11-08/a.journal",
    "backfill/control/a.lock",
    "live/2026-11-08/../x.journal",
    "live/2026-11-08/a/b.journal",
    "live/2026-1-08/a.journal",
    "live/2026-11-08/a.txt",
    "archive/2026/11/x.journal",
    "/live/2026-11-08/a.journal",
    "live/2026-11-08/.journal",
  ] {
    assert!(!valid_live_path(bad), "{bad:?}");
  }
  assert_eq!(
    archive_path("archive/2026/11/2026-11-08.journal.zst"),
    Some(("2026-11-08", false))
  );
  assert_eq!(
    archive_path("backfill/archive/2026/11/2026-11-08.journal.zst"),
    Some(("2026-11-08", true))
  );
  for bad in [
    "archive/2026/12/2026-11-08.journal.zst",
    "archive/2025/11/2026-11-08.journal.zst",
    "archive/2026/11/2026-11-08.journal",
    "archive/2026/11/../2026-11-08.journal.zst",
    "archive/2026/11/manifest",
  ] {
    assert_eq!(archive_path(bad), None, "{bad:?}");
  }
  let sha = "ab".repeat(32);
  let files = vec![
    File {
      path: "live/2026-11-08/a-1.journal".into(),
      size: 42,
      sha256: None,
    },
    File {
      path: "archive/2026/11/2026-11-07.journal.zst".into(),
      size: 9,
      sha256: Some(sha.clone()),
    },
  ];
  let text = manifest::encode(&files);
  assert_eq!(
    text,
    format!(
      "journal-manifest 0.0\nlive/2026-11-08/a-1.journal 42\n\
       archive/2026/11/2026-11-07.journal.zst 9 {sha}\n"
    )
  );
  assert_eq!(manifest::decode(&text), Some(files.clone()));
  // An entry kind this version doesn't know is skipped.
  let body = text.strip_prefix("journal-manifest 0.0\n").unwrap();
  let extra = format!("journal-manifest 0.0\narchive/2026/11 9\n{body}");
  assert_eq!(manifest::decode(&extra), Some(files));
  // An archive needs a whole SHA-256.
  let short =
    "journal-manifest 0.0\narchive/2026/11/2026-11-07.journal.zst 9 ab\n";
  assert_eq!(manifest::decode(short), None);
  for bad in [
    // Within major 0, only the exact version is read.
    "journal-manifest 0.1\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest 1.0\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest 2.0\n",
    "journal-manifest 0.0\nlive/2026-11-08/a-1.journal x\n",
    "journal-manifest 0.0",
  ] {
    assert_eq!(manifest::decode(bad), None, "{bad:?}");
  }
}

#[::tokio::test]
async fn file_endpoint() {
  let root = TempRoot::new();
  let mut writer = Writer::new(root.0.clone(), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  drop(writer);
  let base = serve(&root).await;
  let client = ::reqwest::Client::new();
  let get = |path: &str, range: Option<&str>| {
    let mut request = client
      .get(format!("{base}/files/{path}"))
      .bearer_auth("secret");
    if let Some(range) = range {
      request = request.header("Range", range);
    }
    request.send()
  };
  let path = "live/1970-01-01/a.journal";
  let whole = get(path, None).await.unwrap();
  assert_eq!(whole.status().as_u16(), 200);
  let whole = whole.text().await.unwrap();
  assert_eq!(whole, "journal 0.1\n0 a 1 frame MSG 1\n");
  let tail = get(path, Some("bytes=12-")).await.unwrap();
  assert_eq!(tail.status().as_u16(), 206);
  assert_eq!(tail.text().await.unwrap(), "0 a 1 frame MSG 1\n");
  let code = |response: ::reqwest::Response| response.status().as_u16();
  assert_eq!(code(get(path, Some("bytes=999-")).await.unwrap()), 416);
  assert_eq!(code(get(path, Some("bytes=1-5")).await.unwrap()), 416);
  assert_eq!(
    code(get("live/1970-01-01/b.journal", None).await.unwrap()),
    404
  );
  for escape in ["live/1970-01-01/..%2F..%2Fx.journal", "control/a.status"] {
    assert_eq!(code(get(escape, None).await.unwrap()), 400, "{escape}");
  }
  let anonymous = client.get(format!("{base}/manifest")).send().await.unwrap();
  assert_eq!(code(anonymous), 401);
}

#[::tokio::test]
async fn pulls_only_what_is_missing() {
  let (remote, local) = (TempRoot::new(), TempRoot::new());
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
  };
  let mut writer = Writer::new(remote.0.clone(), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  writer
    .write(86_400_000, 1, Body::Frame("MSG 2".into()))
    .unwrap();
  drop(writer);

  let first = sync(&peer, &local.0).await.unwrap();
  assert_eq!((first.files, first.updated), (2, 2));
  let same = |path: &str| {
    let read = |root: &TempRoot| fs::read(root.0.join(path)).unwrap();
    assert_eq!(read(&local), read(&remote), "{path}");
  };
  same("live/1970-01-01/a.journal");
  same("live/1970-01-02/a.journal");

  // The remote file grows, ending mid-line: only the new bytes come over,
  // the unfinished line included, and the next round finishes it.
  let path = "live/1970-01-02/a.journal";
  let append = |bytes: &[u8]| {
    let mut file = OpenOptions::new()
      .append(true)
      .open(remote.0.join(path))
      .unwrap();
    file.write_all(bytes).unwrap();
  };
  append(b"86400001 a 1 frame MSG");
  let second = sync(&peer, &local.0).await.unwrap();
  assert_eq!((second.updated, second.bytes), (1, 22));
  same(path);
  append(b" 3\n");
  sync(&peer, &local.0).await.unwrap();
  same(path);
  let idle = sync(&peer, &local.0).await.unwrap();
  assert_eq!((idle.updated, idle.bytes), (0, 0));

  // A file shorter remotely than here was rewritten: reported, untouched.
  let before = fs::read(local.0.join(path)).unwrap();
  fs::write(remote.0.join(path), "journal 0.1\n").unwrap();
  let rewritten = sync(&peer, &local.0).await.unwrap();
  assert_eq!(rewritten.mismatched, [path]);
  assert_eq!(fs::read(local.0.join(path)).unwrap(), before);
}

#[::tokio::test]
async fn pulls_backfill_files() {
  let (remote, local) = (TempRoot::new(), TempRoot::new());
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
  };
  let mut writer = Writer::backfill(remote.0.clone(), "backfill-a").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 0,
    to: 5,
  };
  writer.write_on(0, 10, 1, range).unwrap();
  drop(writer);
  let stats = sync(&peer, &local.0).await.unwrap();
  assert_eq!((stats.files, stats.updated), (1, 1));
  let path = "backfill/live/1970-01-01/backfill-a.journal";
  assert_eq!(
    fs::read(local.0.join(path)).unwrap(),
    fs::read(remote.0.join(path)).unwrap()
  );
}

/// Compacts 1970-01-01 under journal `root`.
fn compact(root: &::std::path::Path) {
  compact_day(root, "1970-01-01", 3).unwrap().unwrap();
}

#[::tokio::test]
async fn takes_archives_without_overwriting() {
  let (remote, local) = (TempRoot::new(), TempRoot::new());
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
  };
  let mut writer = Writer::new(remote.0.clone(), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  drop(writer);
  compact(&remote.0);
  let path = "archive/1970/01/1970-01-01.journal.zst";

  // Nothing here for the day: installed as it is, and not taken again.
  let stats = sync(&peer, &local.0).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (1, 0));
  assert_eq!(
    fs::read(local.0.join(path)).unwrap(),
    fs::read(remote.0.join(path)).unwrap()
  );
  assert_eq!(
    ::logger_journal::archive::all(&local.0).unwrap(),
    ::logger_journal::archive::all(&remote.0).unwrap()
  );
  let stats = sync(&peer, &local.0).await.unwrap();
  assert_eq!((stats.installed, stats.merging, stats.bytes), (0, 0, 0));

  // The remote merges more into its archive: this one is kept, and the new
  // one written out as a live file for compaction to merge, once.
  let mut writer = Writer::new(remote.0.clone(), "b");
  writer.write(5, 1, Body::Frame("MSG 2".into())).unwrap();
  drop(writer);
  compact(&remote.0);
  let before = fs::read(local.0.join(path)).unwrap();
  let stats = sync(&peer, &local.0).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (0, 1));
  assert_eq!(fs::read(local.0.join(path)).unwrap(), before);
  let day = local.0.join("live/1970-01-01");
  assert_eq!(fs::read_dir(&day).unwrap().count(), 1);
  compact(&local.0);
  assert_eq!(
    ::logger_journal::archive::all(&local.0).unwrap()[0].lines,
    2
  );
  let taken = fs::read_to_string(local.0.join(TAKEN)).unwrap();
  assert_eq!(taken.lines().count(), 2);
  let stats = sync(&peer, &local.0).await.unwrap();
  assert_eq!((stats.installed, stats.merging, stats.bytes), (0, 0, 0));
}

#[::tokio::test]
async fn takes_backfill_archives() {
  let (remote, local) = (TempRoot::new(), TempRoot::new());
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
  };
  let mut writer = Writer::backfill(remote.0.clone(), "backfill-a").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 0,
    to: 5,
  };
  writer.write_on(0, 10, 1, range).unwrap();
  drop(writer);
  compact(&remote.0.join("backfill"));
  let stats = sync(&peer, &local.0).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (1, 0));
  let path = "backfill/archive/1970/01/1970-01-01.journal.zst";
  assert_eq!(
    fs::read(local.0.join(path)).unwrap(),
    fs::read(remote.0.join(path)).unwrap()
  );
}

/// Serves `routes` on a free port; returns its URL.
async fn serve_routes(routes: ::axum::Router) -> String {
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let url = format!("http://{}", listener.local_addr().unwrap());
  ::tokio::spawn(async move { ::axum::serve(listener, routes).await.unwrap() });
  url
}

#[::tokio::test]
async fn takes_no_more_than_the_manifest_says() {
  use ::axum::{http::StatusCode, response::Redirect, routing::get};
  let local = TempRoot::new();
  let sync_from = |routes: ::axum::Router| async {
    let peer = Peer {
      url: serve_routes(routes).await,
      token: "secret".into(),
    };
    sync(&peer, &local.0).await
  };
  let manifest = |text: String| {
    let text = format!("journal-manifest 0.0\n{text}");
    ::axum::Router::new().route("/manifest", get(move || async { text }))
  };

  // A file sent longer than listed: only what was listed is taken.
  let routes = manifest("live/2026-11-08/a.journal 5\n".into()).route(
    "/files/{*path}",
    get(|| async { (StatusCode::PARTIAL_CONTENT, "x".repeat(100_000)) }),
  );
  let stats = sync_from(routes).await.unwrap();
  assert_eq!((stats.updated, stats.bytes), (1, 5));
  let path = local.0.join("live/2026-11-08/a.journal");
  assert_eq!(fs::read(path).unwrap(), b"xxxxx");

  // A file listed as too big to take.
  let huge = MAX_FILE + 1;
  let error =
    sync_from(manifest(format!("live/2026-11-09/a.journal {huge}\n")))
      .await
      .unwrap_err();
  assert!(error.contains("listed as over"), "{error}");

  // An archive that isn't what the manifest says: nothing is kept.
  let sha = "ab".repeat(32);
  let routes =
    manifest(format!("archive/2026/11/2026-11-07.journal.zst 3 {sha}\n"))
      .route("/files/{*path}", get(|| async { "abc" }));
  let error = sync_from(routes).await.unwrap_err();
  assert!(error.contains("doesn't match"), "{error}");
  assert_eq!(fs::read_dir(local.0.join("control")).unwrap().count(), 0);
  assert!(!local.0.join("archive").exists());

  // A redirect isn't followed: the token would go with it.
  let routes = ::axum::Router::new().route(
    "/manifest",
    get(|| async { Redirect::temporary("/elsewhere") }),
  );
  let error = sync_from(routes).await.unwrap_err();
  assert!(error.contains("307"), "{error}");
}

#[test]
fn refuses_a_decompression_bomb() {
  // Zeros compress about 30,000 to one: a readable header, then more than
  // the limit of them, makes a tiny archive.
  let mut encoder = ::zstd::Encoder::new(Vec::new(), 3).unwrap();
  encoder
    .write_all(
      b"journal 0.1
",
    )
    .unwrap();
  encoder.write_all(&vec![0; 8 << 20]).unwrap();
  let bomb = encoder.finish().unwrap();
  assert!(bomb.len() < 4 << 10, "{}", bomb.len());
  let root = TempRoot::new();
  fs::create_dir_all(&root.0).unwrap();
  let download = root.0.join("bomb.zst");
  fs::write(&download, &bomb).unwrap();
  let error = unpack(&download, None, 1 << 20).unwrap_err();
  assert!(error.to_string().contains("over 1048576 bytes"), "{error}");
  assert_eq!(
    unpack(&download, None, 16 << 20).unwrap(),
    ((8 << 20) + 12, 0)
  );
}
