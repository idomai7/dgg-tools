use {
  ::common::temp_dir::TempDir,
  ::logger::{
    capture::Peer,
    cert,
    compact::compact_day,
    manifest::{self, File, archive_path, valid_live_path},
    serve::{self, Tokens, router},
    sync::{MAX_FILE, TAKEN, sync, unpack},
  },
  ::logger_journal::{Writer, record::v0::Body},
  ::std::{
    fs::{self, OpenOptions},
    io::Write,
  },
};

/// Serves `root` on a free port; returns its URL.
async fn serve(root: &TempDir) -> String {
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let url = format!("http://{}", listener.local_addr().unwrap());
  let tokens = Tokens {
    read: "secret".into(),
    control: None,
  };
  let app = router(root.to_path_buf(), tokens);
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
      "journal-manifest dgg.0.1\nlive/2026-11-08/a-1.journal 42\n\
       archive/2026/11/2026-11-07.journal.zst 9 {sha}\n"
    )
  );
  assert_eq!(manifest::decode(&text), Some(files.clone()));
  // An entry kind this version doesn't know is skipped.
  let body = text.strip_prefix("journal-manifest dgg.0.1\n").unwrap();
  let extra =
    format!("journal-manifest dgg.0.1 ; note=x\narchive/2026/11 9\n{body}");
  assert_eq!(manifest::decode(&extra), Some(files));
  // An archive needs a whole SHA-256.
  let short =
    "journal-manifest dgg.0.1\narchive/2026/11/2026-11-07.journal.zst 9 ab\n";
  assert_eq!(manifest::decode(short), None);
  for bad in [
    // Within major 0, only the exact variant and version are read.
    "journal-manifest 0.0\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest dgg.0.2\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest dgg.1.0\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest kick.0.1\n",
    "journal-manifest dgg.0.1\nlive/2026-11-08/a-1.journal x\n",
    "journal-manifest dgg.0.1",
  ] {
    assert_eq!(manifest::decode(bad), None, "{bad:?}");
  }
}

#[::tokio::test]
async fn file_endpoint() {
  let root = TempDir::new("logger-sync");
  let mut writer = Writer::new(root.to_path_buf(), "a");
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
  assert_eq!(whole, "journal dgg.0.2\n0 a 1 frame MSG 1\u{1e}\n");
  let tail = get(path, Some("bytes=16-")).await.unwrap();
  assert_eq!(tail.status().as_u16(), 206);
  assert_eq!(tail.text().await.unwrap(), "0 a 1 frame MSG 1\u{1e}\n");
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

/// Serves `root` over TLS on a free port, with a certificate for
/// 127.0.0.1 from a new CA; returns its URL and the CA's certificate.
async fn serve_tls(root: &TempDir) -> (String, String) {
  let key = ::rcgen::KeyPair::generate().unwrap();
  let ca_cert = cert::ca(&key).unwrap();
  let node = cert::node("node", &["127.0.0.1".into()], &ca_cert, &key).unwrap();
  let tls = cert::server_config(&node.cert, &node.key).unwrap();
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let url = format!("https://{}", listener.local_addr().unwrap());
  let tokens = Tokens {
    read: "secret".into(),
    control: None,
  };
  let app = router(root.to_path_buf(), tokens);
  ::tokio::spawn(
    async move { serve::run(listener, app, Some(tls)).await.unwrap() },
  );
  (url, ca_cert)
}

#[::tokio::test]
async fn pulls_over_tls_from_its_ca_alone() {
  let (remote, local) =
    (TempDir::new("logger-sync"), TempDir::new("logger-sync"));
  let (url, ca_cert) = serve_tls(&remote).await;
  let mut writer = Writer::new(remote.to_path_buf(), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  drop(writer);
  let peer = Peer {
    url: url.clone(),
    token: "secret".into(),
    ca: Some(ca_cert),
  };
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.files, stats.updated), (1, 1));
  // Not from a server another CA vouches for, nor the system's roots.
  let other = cert::ca(&::rcgen::KeyPair::generate().unwrap()).unwrap();
  let refused = |ca: Option<String>| Peer {
    url: url.clone(),
    token: "secret".into(),
    ca,
  };
  assert!(sync(&refused(Some(other)), local.path()).await.is_err());
  assert!(sync(&refused(None), local.path()).await.is_err());
  // And with a CA, never over plain HTTP.
  let plain = Peer {
    url: serve(&remote).await,
    ..peer
  };
  assert!(sync(&plain, local.path()).await.is_err());
}

#[::tokio::test]
async fn pulls_only_what_is_missing() {
  let (remote, local) =
    (TempDir::new("logger-sync"), TempDir::new("logger-sync"));
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
    ca: None,
  };
  let mut writer = Writer::new(remote.to_path_buf(), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  writer
    .write(86_400_000, 1, Body::Frame("MSG 2".into()))
    .unwrap();
  drop(writer);

  let first = sync(&peer, local.path()).await.unwrap();
  assert_eq!((first.files, first.updated), (2, 2));
  let same = |path: &str| {
    let read = |root: &TempDir| fs::read(root.path().join(path)).unwrap();
    assert_eq!(read(&local), read(&remote), "{path}");
  };
  same("live/1970-01-01/a.journal");
  same("live/1970-01-02/a.journal");

  // The remote file grows, ending mid-record: only the new bytes come over,
  // the unfinished record included, and the next round finishes it.
  let path = "live/1970-01-02/a.journal";
  let append = |bytes: &[u8]| {
    let mut file = OpenOptions::new()
      .append(true)
      .open(remote.path().join(path))
      .unwrap();
    file.write_all(bytes).unwrap();
  };
  append(b"86400001 a 1 frame MSG");
  let second = sync(&peer, local.path()).await.unwrap();
  assert_eq!((second.updated, second.bytes), (1, 22));
  same(path);
  append(b" 3\n");
  sync(&peer, local.path()).await.unwrap();
  same(path);
  let idle = sync(&peer, local.path()).await.unwrap();
  assert_eq!((idle.updated, idle.bytes), (0, 0));

  // A file shorter remotely than here was rewritten: reported, untouched.
  let before = fs::read(local.path().join(path)).unwrap();
  fs::write(remote.path().join(path), "journal 0.1\n").unwrap();
  let rewritten = sync(&peer, local.path()).await.unwrap();
  assert_eq!(rewritten.mismatched, [path]);
  assert_eq!(fs::read(local.path().join(path)).unwrap(), before);
}

#[::tokio::test]
async fn pulls_backfill_files() {
  let (remote, local) =
    (TempDir::new("logger-sync"), TempDir::new("logger-sync"));
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
    ca: None,
  };
  let mut writer =
    Writer::backfill(remote.to_path_buf(), "backfill-a").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 0,
    to: 5,
  };
  writer.write_on(0, 10, 1, range).unwrap();
  drop(writer);
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.files, stats.updated), (1, 1));
  let path = "backfill/live/1970-01-01/backfill-a.journal";
  assert_eq!(
    fs::read(local.path().join(path)).unwrap(),
    fs::read(remote.path().join(path)).unwrap()
  );
}

/// Compacts 1970-01-01 under journal `root`.
fn compact(root: &::std::path::Path) {
  compact_day(root, "1970-01-01", 3).unwrap().unwrap();
}

#[::tokio::test]
async fn takes_archives_without_overwriting() {
  let (remote, local) =
    (TempDir::new("logger-sync"), TempDir::new("logger-sync"));
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
    ca: None,
  };
  let mut writer = Writer::new(remote.to_path_buf(), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  drop(writer);
  compact(remote.path());
  let path = "archive/1970/01/1970-01-01.journal.zst";

  // Nothing here for the day: installed as it is, and not taken again.
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (1, 0));
  assert_eq!(
    fs::read(local.path().join(path)).unwrap(),
    fs::read(remote.path().join(path)).unwrap()
  );
  assert_eq!(
    ::logger_journal::archive::all(local.path()).unwrap(),
    ::logger_journal::archive::all(remote.path()).unwrap()
  );
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging, stats.bytes), (0, 0, 0));

  // The remote merges more into its archive: this one is kept, and the new
  // one written out as a live file for compaction to merge, once.
  let mut writer = Writer::new(remote.to_path_buf(), "b");
  writer.write(5, 1, Body::Frame("MSG 2".into())).unwrap();
  drop(writer);
  compact(remote.path());
  let before = fs::read(local.path().join(path)).unwrap();
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (0, 1));
  assert_eq!(fs::read(local.path().join(path)).unwrap(), before);
  let day = local.path().join("live/1970-01-01");
  assert_eq!(fs::read_dir(&day).unwrap().count(), 1);
  compact(local.path());
  assert_eq!(
    ::logger_journal::archive::all(local.path()).unwrap()[0].records,
    2
  );
  let taken = fs::read_to_string(local.path().join(TAKEN)).unwrap();
  assert_eq!(taken.lines().count(), 2);
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging, stats.bytes), (0, 0, 0));
}

#[::tokio::test]
async fn takes_backfill_archives() {
  let (remote, local) =
    (TempDir::new("logger-sync"), TempDir::new("logger-sync"));
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
    ca: None,
  };
  let mut writer =
    Writer::backfill(remote.to_path_buf(), "backfill-a").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 0,
    to: 5,
  };
  writer.write_on(0, 10, 1, range).unwrap();
  drop(writer);
  compact(&remote.path().join("backfill"));
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (1, 0));
  let path = "backfill/archive/1970/01/1970-01-01.journal.zst";
  assert_eq!(
    fs::read(local.path().join(path)).unwrap(),
    fs::read(remote.path().join(path)).unwrap()
  );
}

/// The records of `root`'s archive of 1970-01-01, sorted.
fn archived_records(root: &::std::path::Path) -> Vec<String> {
  let path = root.join("archive/1970/01/1970-01-01.journal.zst");
  let bytes = ::zstd::decode_all(fs::File::open(path).unwrap()).unwrap();
  let text = String::from_utf8(bytes).unwrap();
  let (header, body) = text.split_once('\n').unwrap();
  assert_eq!(header, "journal dgg.0.2");
  let mut records: Vec<String> = body
    .split_terminator("\u{1e}\n")
    .map(str::to_owned)
    .collect();
  records.sort();
  records
}

#[::tokio::test]
async fn a_mesh_settles() {
  // Three nodes capturing the same day, at the same times, so each can
  // order its archive's ties differently.
  let roots = [
    TempDir::new("logger-sync"),
    TempDir::new("logger-sync"),
    TempDir::new("logger-sync"),
  ];
  let mut urls = Vec::new();
  for (n, root) in roots.iter().enumerate() {
    let mut writer = Writer::new(root.to_path_buf(), format!("node{n}"));
    for at in [0, 5, 10] {
      let frame = format!("MSG {n}-{at}");
      writer.write(at, 1, Body::Frame(frame)).unwrap();
    }
    drop(writer);
    // Compacted before hearing from the others: each starts with an
    // archive the others' differ from.
    compact_day(root.path(), "1970-01-01", 3).unwrap();
    urls.push(serve(root).await);
  }
  let peer = |n: usize| Peer {
    url: urls[n].clone(),
    token: "secret".into(),
    ca: None,
  };

  // Rounds of every node syncing from both others in turn, then
  // compacting, until a round moves nothing.
  let mut rounds = 0;
  loop {
    rounds += 1;
    assert!(rounds <= 5, "still moving after {} rounds", rounds - 1);
    let mut moved = 0;
    for (n, root) in roots.iter().enumerate() {
      for other in (0..3).filter(|&other| other != n) {
        moved += sync(&peer(other), root.path()).await.unwrap().bytes;
      }
    }
    for root in &roots {
      compact_day(root.path(), "1970-01-01", 3).unwrap();
    }
    if moved == 0 {
      break;
    }
  }
  // Each node holds every record once, whatever order its ties are in.
  let all = archived_records(&roots[0]);
  assert_eq!(all.len(), 9);
  for root in &roots[1..] {
    assert_eq!(archived_records(root.path()), all);
  }
  // A settled mesh stays settled.
  for (n, root) in roots.iter().enumerate() {
    for other in (0..3).filter(|&other| other != n) {
      let stats = sync(&peer(other), root.path()).await.unwrap();
      assert_eq!((stats.bytes, stats.merging), (0, 0));
    }
  }
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
  let local = TempDir::new("logger-sync");
  let sync_from = |routes: ::axum::Router| async {
    let peer = Peer {
      url: serve_routes(routes).await,
      token: "secret".into(),
      ca: None,
    };
    sync(&peer, local.path()).await
  };
  let manifest = |text: String| {
    let text = format!("journal-manifest dgg.0.1\n{text}");
    ::axum::Router::new().route("/manifest", get(move || async { text }))
  };

  // A file sent longer than listed: only what was listed is taken.
  let routes = manifest("live/2026-11-08/a.journal 5\n".into()).route(
    "/files/{*path}",
    get(|| async { (StatusCode::PARTIAL_CONTENT, "x".repeat(100_000)) }),
  );
  let stats = sync_from(routes).await.unwrap();
  assert_eq!((stats.updated, stats.bytes), (1, 5));
  let path = local.path().join("live/2026-11-08/a.journal");
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
  assert_eq!(
    fs::read_dir(local.path().join("control")).unwrap().count(),
    0
  );
  assert!(!local.path().join("archive").exists());

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
  let root = TempDir::new("logger-sync");
  fs::create_dir_all(root.path()).unwrap();
  let download = root.path().join("bomb.zst");
  fs::write(&download, &bomb).unwrap();
  let error = unpack(&download, None, 1 << 20).unwrap_err();
  assert!(error.to_string().contains("over 1048576 bytes"), "{error}");
  assert_eq!(
    unpack(&download, None, 16 << 20).unwrap(),
    ((8 << 20) + 12, 0)
  );
}

#[test]
fn counts_records_by_framing() {
  let root = TempDir::new("logger-sync");
  fs::create_dir_all(root.path()).unwrap();
  let count = |journal: &[u8]| {
    let path = root.path().join("archive.zst");
    fs::write(&path, ::zstd::encode_all(journal, 3).unwrap()).unwrap();
    unpack(&path, None, 16 << 20).unwrap()
  };
  // Records spanning lines, and an RS LF across where reads may split.
  let header = b"journal dgg.0.2\n";
  let mut journal = header.to_vec();
  journal.extend(b"0 a 1 frame A\nB\n\x1e\n");
  let filler = (64 << 10) - journal.len() - 1;
  journal.extend(b"0 a 1 frame ");
  journal.resize(journal.len() + filler - 12, b'x');
  journal.extend(b"\x1e\n0 a 1 frame C\x1e\n");
  assert_eq!(count(&journal), (journal.len() as u64, 3));
  // Before, a record a line.
  let legacy = b"journal 0.1\n0 a 1 frame A\n0 a 1 frame B\n";
  assert_eq!(count(legacy), (legacy.len() as u64, 2));
}
