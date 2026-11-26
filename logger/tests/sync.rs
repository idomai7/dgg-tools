use {
  ::common::temp_dir::TempDir,
  ::logger::{
    capture::Peer,
    cert,
    compact::compact_day,
    dgg::{Dgg, channels::DESTINY},
    kick::Kick,
    manifest::{self, File, archive_path, live_path, valid_path},
    serve::{self, Tokens, router},
    service::{Root, Service},
    sync::{MAX_FILE, TAKEN, sync, unpack},
  },
  ::logger_journal::{
    Writer,
    layout::{channel_root, kick_root},
    record::{Variant, kick, v0::Body},
    writer::kick::Writer as KickWriter,
  },
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
    query: None,
  };
  let app = router(root.to_path_buf(), tokens);
  ::tokio::spawn(async move { ::axum::serve(listener, app).await.unwrap() });
  url
}

#[test]
fn manifest_format() {
  assert!(live_path("live/2026-11-08/a-1.journal"));
  assert!(valid_path::<Dgg>("live/2026-11-08/a-1.journal"));
  assert!(valid_path::<Dgg>(
    "backfill/live/2026-11-08/backfill-a.journal"
  ));
  assert!(valid_path::<Dgg>(
    "backfill/archive/2026/11/2026-11-08.journal.zst"
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
    assert!(!valid_path::<Dgg>(bad), "{bad:?}");
  }
  assert_eq!(
    archive_path("archive/2026/11/2026-11-08.journal.zst"),
    Some("2026-11-08")
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
  let text = manifest::encode::<Dgg>(&files);
  assert_eq!(
    text,
    format!(
      "journal-manifest dgg.0.2\u{1e}\nlive/2026-11-08/a-1.journal 42\u{1e}\n\
       archive/2026/11/2026-11-07.journal.zst 9 {sha}\u{1e}\n"
    )
  );
  assert_eq!(manifest::decode::<Dgg>(&text), Some(files.clone()));
  // An entry kind this version doesn't know is skipped.
  let body = text
    .strip_prefix("journal-manifest dgg.0.2\u{1e}\n")
    .unwrap();
  let extra = format!(
    "journal-manifest dgg.0.2 ; note=x\u{1e}\narchive/2026/11 9\u{1e}\n{body}"
  );
  assert_eq!(manifest::decode::<Dgg>(&extra), Some(files));
  // An archive needs a whole SHA-256.
  let short = "journal-manifest dgg.0.2\u{1e}\n\
               archive/2026/11/2026-11-07.journal.zst 9 ab\u{1e}\n";
  assert_eq!(manifest::decode::<Dgg>(short), None);
  for bad in [
    // Within major 0, only the exact variant and version are read.
    "journal-manifest 0.0\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest dgg.0.2\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest dgg.1.0\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest kick.0.1\n",
    "journal-manifest dgg.0.2\u{1e}\nlive/2026-11-08/a-1.journal x\u{1e}\n",
    // An entry without its end.
    "journal-manifest dgg.0.2\u{1e}\nlive/2026-11-08/a-1.journal 42\n",
    "journal-manifest dgg.0.1",
  ] {
    assert_eq!(manifest::decode::<Dgg>(bad), None, "{bad:?}");
  }
}

#[::tokio::test]
async fn file_endpoint() {
  let root = TempDir::new("logger-sync");
  let mut writer = Writer::new(dgg(&root), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  drop(writer);
  let base = serve(&root).await;
  let client = ::reqwest::Client::new();
  let get = |path: &str, range: Option<&str>| {
    let mut request = client
      .get(format!("{base}/dgg/v0/channels/destiny.gg/files/{path}"))
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
  assert_eq!(whole, "journal dgg.0.3\u{1e}\n0 a 1 frame MSG 1\u{1e}\n");
  let tail = get(path, Some("bytes=17-")).await.unwrap();
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
  let anonymous = client
    .get(format!("{base}/dgg/v0/channels/destiny.gg/manifest"))
    .send()
    .await
    .unwrap();
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
    query: None,
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
  let mut writer = Writer::new(dgg(&remote), "a");
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
  let mut writer = Writer::new(dgg(&remote), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  writer
    .write(86_400_000, 1, Body::Frame("MSG 2".into()))
    .unwrap();
  drop(writer);

  let first = sync(&peer, local.path()).await.unwrap();
  assert_eq!((first.files, first.updated), (2, 2));
  let same = |path: &str| {
    let read = |root: &TempDir| fs::read(dgg(root).join(path)).unwrap();
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
      .open(dgg(&remote).join(path))
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
  let before = fs::read(dgg(&local).join(path)).unwrap();
  fs::write(dgg(&remote).join(path), "journal 0.1\n").unwrap();
  let rewritten = sync(&peer, local.path()).await.unwrap();
  assert_eq!(rewritten.mismatched, [format!("dgg destiny.gg: {path}")]);
  assert_eq!(fs::read(dgg(&local).join(path)).unwrap(), before);
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
  let mut writer = Writer::backfill(dgg(&remote), "backfill-a").unwrap();
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
    fs::read(dgg(&local).join(path)).unwrap(),
    fs::read(dgg(&remote).join(path)).unwrap()
  );
}

/// destiny.gg's journal root in `root`'s journal directory.
fn dgg(root: &TempDir) -> ::std::path::PathBuf {
  Dgg::channel_dir(&Dgg::dir(root.path()), DESTINY)
}

/// Compacts 1970-01-01 under journal `root`.
fn compact(root: &::std::path::Path) {
  compact_day(&Root::at(root), "1970-01-01", 3)
    .unwrap()
    .unwrap();
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
  let mut writer = Writer::new(dgg(&remote), "a");
  writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  drop(writer);
  compact(&dgg(&remote));
  let path = "archive/1970/01/1970-01-01.journal.zst";

  // Nothing here for the day: installed as it is, and not taken again.
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (1, 0));
  assert_eq!(
    fs::read(dgg(&local).join(path)).unwrap(),
    fs::read(dgg(&remote).join(path)).unwrap()
  );
  assert_eq!(
    ::logger_journal::archive::all(&dgg(&local)).unwrap(),
    ::logger_journal::archive::all(&dgg(&remote)).unwrap()
  );
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging, stats.bytes), (0, 0, 0));

  // The remote merges more into its archive: this one is kept, and the new
  // one written out as a live file for compaction to merge, once.
  let mut writer = Writer::new(dgg(&remote), "b");
  writer.write(5, 1, Body::Frame("MSG 2".into())).unwrap();
  drop(writer);
  compact(&dgg(&remote));
  let before = fs::read(dgg(&local).join(path)).unwrap();
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (0, 1));
  assert_eq!(fs::read(dgg(&local).join(path)).unwrap(), before);
  let day = dgg(&local).join("live/1970-01-01");
  assert_eq!(fs::read_dir(&day).unwrap().count(), 1);
  compact(&dgg(&local));
  assert_eq!(
    ::logger_journal::archive::all(&dgg(&local)).unwrap()[0].records,
    2
  );
  let taken = fs::read_to_string(dgg(&local).join(TAKEN)).unwrap();
  assert!(
    taken.starts_with("synced-archives dgg.0.2\u{1e}\n"),
    "{taken}"
  );
  assert_eq!(
    ::logger::taken::read(&dgg(&local), Variant::Dgg)
      .unwrap()
      .len(),
    2
  );
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
  let mut writer = Writer::backfill(dgg(&remote), "backfill-a").unwrap();
  let range = Body::FetchRange {
    source: "x".into(),
    from: 0,
    to: 5,
  };
  writer.write_on(0, 10, 1, range).unwrap();
  drop(writer);
  compact(&dgg(&remote).join("backfill"));
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (1, 0));
  let path = "backfill/archive/1970/01/1970-01-01.journal.zst";
  assert_eq!(
    fs::read(dgg(&local).join(path)).unwrap(),
    fs::read(dgg(&remote).join(path)).unwrap()
  );
}

/// The records of `root`'s archive of 1970-01-01, sorted.
fn archived_records(root: &::std::path::Path) -> Vec<String> {
  let path = Dgg::channel_dir(&Dgg::dir(root), DESTINY)
    .join("archive/1970/01/1970-01-01.journal.zst");
  let bytes = ::zstd::decode_all(fs::File::open(path).unwrap()).unwrap();
  let text = String::from_utf8(bytes).unwrap();
  // The header's a record like the rest.
  let mut records: Vec<String> = text
    .split_terminator("\u{1e}\n")
    .map(str::to_owned)
    .collect();
  assert_eq!(records.remove(0), "journal dgg.0.3");
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
    let mut writer = Writer::new(dgg(root), format!("node{n}"));
    for at in [0, 5, 10] {
      let frame = format!("MSG {n}-{at}");
      writer.write(at, 1, Body::Frame(frame)).unwrap();
    }
    drop(writer);
    // Compacted before hearing from the others: each starts with an
    // archive the others' differ from.
    compact_day(&Root::at(dgg(root)), "1970-01-01", 3).unwrap();
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
      compact_day(&Root::at(dgg(root)), "1970-01-01", 3).unwrap();
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
  let at = "/dgg/v0/channels/destiny.gg";
  let manifest = |text: String| {
    // Each entry ends at RS LF.
    let text = format!(
      "journal-manifest dgg.0.2\u{1e}\n{}",
      text.replace('\n', "\u{1e}\n")
    );
    ::axum::Router::new()
      .route("/dgg/v0/channels", get(|| async { "destiny.gg\n" }))
      .route(&format!("{at}/manifest"), get(move || async { text }))
  };

  // A file sent longer than listed: only what was listed is taken.
  let routes = manifest("live/2026-11-08/a.journal 5\n".into()).route(
    &format!("{at}/files/{{*path}}"),
    get(|| async {
      let range = [("content-range", "bytes 0-99999/100000")];
      (StatusCode::PARTIAL_CONTENT, range, "x".repeat(100_000))
    }),
  );
  let stats = sync_from(routes).await.unwrap();
  assert_eq!((stats.updated, stats.bytes), (1, 5));
  let path = dgg(&local).join("live/2026-11-08/a.journal");
  assert_eq!(fs::read(&path).unwrap(), b"xxxxx");

  // A file sent from anywhere but where it was asked for: nothing's taken.
  let routes = manifest(
    "live/2026-11-08/a.journal 10
"
    .into(),
  )
  .route(
    &format!("{at}/files/{{*path}}"),
    get(|| async { (StatusCode::PARTIAL_CONTENT, "yyyyyyyyyy") }),
  );
  let error = sync_from(routes).await.unwrap_err();
  assert!(error.contains("not sent from byte 5"), "{error}");
  assert_eq!(fs::read(&path).unwrap(), b"xxxxx");

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
      .route(&format!("{at}/files/{{*path}}"), get(|| async { "abc" }));
  let error = sync_from(routes).await.unwrap_err();
  assert!(error.contains("doesn't match"), "{error}");
  assert_eq!(
    fs::read_dir(dgg(&local).join("control")).unwrap().count(),
    0
  );
  assert!(!dgg(&local).join("archive").exists());

  // A redirect isn't followed: the token would go with it.
  let routes = ::axum::Router::new().route(
    "/dgg/v0/channels",
    get(|| async { Redirect::temporary("/elsewhere") }),
  );
  let error = sync_from(routes).await.unwrap_err();
  assert!(error.contains("307"), "{error}");

  // A channel's name that isn't one.
  let routes = ::axum::Router::new()
    .route("/dgg/v0/channels", get(|| async { "destiny.gg\n..\n" }));
  let error = sync_from(routes).await.unwrap_err();
  assert!(error.contains(r#""..""#), "{error}");
}

#[::tokio::test]
async fn takes_nothing_from_a_build_before_channels() {
  use ::axum::routing::get;
  let local = TempDir::new("logger-sync");
  // Its whole service's manifest, which this build never asks for.
  let text =
    "journal-manifest dgg.0.2\u{1e}\nlive/2026-11-08/a.journal 5\u{1e}\n";
  let routes = ::axum::Router::new()
    .route("/dgg/v0/manifest", get(move || async move { text }))
    .route("/dgg/v0/files/{*path}", get(|| async { "xxxxx" }));
  let peer = Peer {
    url: serve_routes(routes).await,
    token: "secret".into(),
    ca: None,
  };
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.files, stats.bytes), (0, 0));
  assert!(!Dgg::dir(local.path()).exists());
}

#[::tokio::test]
async fn lists_each_services_channels() {
  let root = TempDir::new("logger-sync");
  kick_records(
    &root,
    "b",
    "k",
    vec![(0, kick::v0::Body::Frame("{}".into()))],
  );
  kick_records(
    &root,
    "a",
    "k",
    vec![(0, kick::v0::Body::Frame("{}".into()))],
  );
  let base = serve(&root).await;
  let client = ::reqwest::Client::new();
  let get = |path: &str| {
    client
      .get(format!("{base}{path}"))
      .bearer_auth("secret")
      .send()
  };
  let text = |path: &'static str| async move {
    let response = get(path).await.unwrap();
    assert_eq!(response.status().as_u16(), 200, "{path}");
    response.text().await.unwrap()
  };
  assert_eq!(text("/dgg/v0/channels").await, "");
  for name in [DESTINY, "chat.example"] {
    let dir = Dgg::channel_dir(&Dgg::dir(root.path()), name);
    let mut writer = Writer::new(dir, "a");
    writer.write(0, 1, Body::Frame("MSG 1".into())).unwrap();
  }
  assert_eq!(text("/dgg/v0/channels").await, "chat.example\ndestiny.gg\n");
  assert_eq!(text("/kick/v0/channels").await, "a\nb\n");
  // Not a channel with journals here, or not a channel's name.
  for (path, expected) in [
    ("/dgg/v0/channels/other.example/manifest", 404),
    ("/dgg/v0/channels/live/control", 400),
    ("/dgg/v0/channels/live/manifest", 400),
    ("/kick/v0/channels/c/manifest", 404),
    ("/kick/v0/channels/a.b/manifest", 400),
    // The routes before channels.
    ("/dgg/v0/manifest", 404),
    ("/kick/v0/manifest", 404),
  ] {
    let response = get(path).await.unwrap();
    assert_eq!(response.status().as_u16(), expected, "{path}");
  }
}

#[test]
fn refuses_a_decompression_bomb() {
  // Zeros compress about 30,000 to one: a readable header, then more than
  // the limit of them, makes a tiny archive.
  let mut encoder = ::zstd::Encoder::new(Vec::new(), 3).unwrap();
  encoder.write_all(b"journal dgg.0.3\x1e\n").unwrap();
  encoder.write_all(&vec![0; 8 << 20]).unwrap();
  let bomb = encoder.finish().unwrap();
  assert!(bomb.len() < 4 << 10, "{}", bomb.len());
  let root = TempDir::new("logger-sync");
  fs::create_dir_all(root.path()).unwrap();
  let download = root.path().join("bomb.zst");
  fs::write(&download, &bomb).unwrap();
  let error = unpack(&download, Variant::Dgg, None, 1 << 20).unwrap_err();
  assert!(error.to_string().contains("over 1048576 bytes"), "{error}");
  assert_eq!(
    unpack(&download, Variant::Dgg, None, 16 << 20).unwrap(),
    ((8 << 20) + 17, 0)
  );
}

#[test]
fn counts_records_by_framing() {
  let root = TempDir::new("logger-sync");
  fs::create_dir_all(root.path()).unwrap();
  let count = |journal: &[u8]| {
    let path = root.path().join("archive.zst");
    fs::write(&path, ::zstd::encode_all(journal, 3).unwrap()).unwrap();
    unpack(&path, Variant::Dgg, None, 16 << 20).unwrap()
  };
  // Records spanning lines, and an RS LF across where reads may split.
  let header = b"journal dgg.0.3\x1e\n";
  let mut journal = header.to_vec();
  journal.extend(b"0 a 1 frame A\nB\n\x1e\n");
  let filler = (64 << 10) - journal.len() - 1;
  journal.extend(b"0 a 1 frame ");
  journal.resize(journal.len() + filler - 12, b'x');
  journal.extend(b"\x1e\n0 a 1 frame C\x1e\n");
  assert_eq!(count(&journal), (journal.len() as u64, 3));
  // Older versions are no longer read.
  let path = root.path().join("archive.zst");
  let legacy = b"journal 0.1\n0 a 1 frame A\n0 a 1 frame B\n";
  fs::write(&path, ::zstd::encode_all(&legacy[..], 3).unwrap()).unwrap();
  assert!(unpack(&path, Variant::Dgg, None, 16 << 20).is_err());
}

/// Writes Kick records for channel `slug` under journal `root`, as instance
/// `instance`, each `(receive time, body)`.
fn kick_records(
  root: &TempDir,
  slug: &str,
  instance: &str,
  records: Vec<(i64, kick::v0::Body)>,
) {
  let channel = channel_root(&kick_root(root.path()), slug);
  let mut writer = KickWriter::new(channel, instance, slug);
  for (time, body) in records {
    writer.write(time, 1, body).unwrap();
  }
}

#[::tokio::test]
async fn pulls_kick_journals() {
  use kick::v0::Body::{Frame, Open};
  let (remote, local) =
    (TempDir::new("logger-sync"), TempDir::new("logger-sync"));
  let peer = Peer {
    url: serve(&remote).await,
    token: "secret".into(),
    ca: None,
  };
  let open = || Open("wss://one".into());
  kick_records(
    &remote,
    "c",
    "k",
    vec![(0, open()), (1, Frame("{}".into()))],
  );
  compact(&channel_root(&kick_root(remote.path()), "c"));
  kick_records(&remote, "c", "k", vec![(86_400_000, open())]);

  // The channel's own manifest, every path relative to it.
  let manifest = ::reqwest::Client::new()
    .get(format!("{}/kick/v0/channels/c/manifest", peer.url))
    .bearer_auth("secret")
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
  let files = manifest::decode::<Kick>(&manifest).unwrap();
  let paths: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
  assert_eq!(
    paths,
    [
      "live/1970-01-02/k.journal",
      "archive/1970/01/1970-01-01.journal.zst"
    ]
  );
  // Not in dgg's manifest.
  assert_eq!(manifest::list::<Dgg>(&dgg(&remote), DESTINY).unwrap(), []);

  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.files, stats.updated, stats.installed), (2, 1, 1));
  for path in &paths {
    let read = |root: &TempDir| {
      fs::read(channel_root(&kick_root(root.path()), "c").join(path)).unwrap()
    };
    assert_eq!(read(&local), read(&remote), "{path}");
  }
  let idle = sync(&peer, local.path()).await.unwrap();
  assert_eq!((idle.updated, idle.bytes), (0, 0));

  // A day both have: the peer's archive is written out for compaction to
  // merge, and recorded as taken in the channel's own file.
  kick_records(&local, "c", "l", vec![(86_400_005, open())]);
  let local_channel = channel_root(&kick_root(local.path()), "c");
  let remote_channel = channel_root(&kick_root(remote.path()), "c");
  compact_day(&Root::at(&local_channel), "1970-01-02", 3)
    .unwrap()
    .unwrap();
  compact_day(&Root::at(&remote_channel), "1970-01-02", 3)
    .unwrap()
    .unwrap();
  let stats = sync(&peer, local.path()).await.unwrap();
  assert_eq!((stats.installed, stats.merging), (0, 1));
  let taken = fs::read_to_string(local_channel.join(TAKEN)).unwrap();
  assert!(
    taken.starts_with("synced-archives kick.0.0\u{1e}\n"),
    "{taken}"
  );
  compact_day(&Root::at(&local_channel), "1970-01-02", 3)
    .unwrap()
    .unwrap();
  let archives = ::logger_journal::archive::all(&local_channel).unwrap();
  assert_eq!(archives[1].records, 2);
}

#[::tokio::test]
async fn serves_only_kick_journal_paths() {
  let root = TempDir::new("logger-sync");
  kick_records(
    &root,
    "c",
    "k",
    vec![(0, kick::v0::Body::Frame("{}".into()))],
  );
  let base = serve(&root).await;
  let client = ::reqwest::Client::new();
  for (path, expected) in [
    ("c/files/live/1970-01-01/k.journal", 200),
    ("C/files/live/1970-01-01/k.journal", 400),
    ("c/files/backfill/live/1970-01-01/k.journal", 400),
    ("c/files/control/k.status", 400),
    ("c/files/c/live/1970-01-01/k.journal", 400),
    ("d/files/live/1970-01-01/k.journal", 404),
  ] {
    let response = client
      .get(format!("{base}/kick/v0/channels/{path}"))
      .bearer_auth("secret")
      .send()
      .await
      .unwrap();
    assert_eq!(response.status().as_u16(), expected, "{path}");
  }
}
