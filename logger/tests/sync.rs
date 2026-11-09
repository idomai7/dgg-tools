use {
  ::journal::{Writer, record::v0::Body},
  ::logger::{
    capture::Peer,
    manifest::{self, File, valid_live_path},
    serve::router,
    sync::sync,
  },
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
  let app = router(root.0.clone(), "secret".into());
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
  let files = vec![File {
    path: "live/2026-11-08/a-1.journal".into(),
    size: 42,
  }];
  let text = manifest::encode(&files);
  assert_eq!(
    text,
    "journal-manifest 0.0\nlive/2026-11-08/a-1.journal 42\n"
  );
  assert_eq!(manifest::decode(&text), Some(files.clone()));
  // An entry kind this version doesn't know is skipped.
  let extra = "journal-manifest 0.0\narchive/2026/11 9\n\
    live/2026-11-08/a-1.journal 42\n";
  assert_eq!(manifest::decode(extra), Some(files));
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
