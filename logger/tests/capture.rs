use {
  ::common::temp_dir::TempDir,
  ::dgg_api::chat::Options,
  ::futures_util::{SinkExt, StreamExt},
  ::logger::{
    capture::{Exit, HANDOFF, Handoff, Peer, SHUTDOWN, instance_id},
    control,
    dgg::{
      Dgg,
      capture::{Config, run},
      channels::DESTINY,
    },
    service::Service,
  },
  ::logger_journal::{
    Reader, Record,
    layout::live_files,
    record::v0::{self, Body},
  },
  ::std::{path::Path, time::Duration},
  ::tokio::{net::TcpListener, sync::oneshot},
  ::tokio_tungstenite::tungstenite::Message,
};

/// destiny.gg's directory, in the journal directory `journal`.
fn destiny(journal: &::std::path::Path) -> ::std::path::PathBuf {
  Dgg::channel_dir(&Dgg::dir(journal), DESTINY)
}

/// A chat server where every connection gets HISTORY, then a MSG every
/// 10 ms, so instances become healthy at once.
async fn chat_server() -> (String, ::tokio::task::JoinHandle<()>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let url = format!("ws://{}", listener.local_addr().unwrap());
  let server = ::tokio::spawn(async move {
    loop {
      let (tcp, _) = listener.accept().await.unwrap();
      ::tokio::spawn(async move {
        let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.send(Message::text("HISTORY []")).await.unwrap();
        while ws.send(Message::text("MSG {}")).await.is_ok() {
          ::tokio::time::sleep(Duration::from_millis(10)).await;
        }
      });
    }
  });
  (url, server)
}

fn quick_handoff() -> Handoff {
  Handoff {
    grace: Duration::from_millis(200),
    stale_after: Duration::from_secs(2),
    interval: Duration::from_millis(20),
    keep_running: false,
  }
}

#[test]
fn instance_ids() {
  assert_eq!(instance_id("deskpc", 5), "deskpc-5");
  assert_eq!(instance_id("my host.lan", 5), "my-host.lan-5");
  assert_eq!(instance_id("", 5), "host-5");
  assert!(v0::valid_instance(&instance_id("π/é:x", 5)));
}

/// Two connections from a local server, then a clean shutdown while the
/// second is open: everything lands in the journal, bracketed by open/close.
#[::tokio::test]
async fn journals_connections_and_shutdown() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let url = format!("ws://{}", listener.local_addr().unwrap());
  let (served, served_rx) = oneshot::channel();
  let server = ::tokio::spawn(async move {
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.send(Message::text("MSG {\"n\":1}")).await.unwrap();
    ws.send(Message::binary(vec![7])).await.unwrap();
    ws.close(None).await.unwrap();
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.send(Message::text("MSG {\"n\":2}")).await.unwrap();
    served.send(()).unwrap();
    // Hold the socket open until capture shuts down.
    while ws.next().await.is_some() {}
  });

  let root = TempDir::new("logger-capture");
  let chat = Options {
    url: url.clone(),
    origin: None,
    min_backoff: Duration::from_millis(10),
    ..Options::default()
  };
  let shutdown = async {
    served_rx.await.unwrap();
    // Let capture take the last frame off its queue.
    ::tokio::time::sleep(Duration::from_millis(100)).await;
  };
  let config = Config {
    chat,
    journal_root: root.to_path_buf(),
    instance: "t".into(),
    started: 1,
    handoff: Handoff::default(),
    peer: None,
    old_control: None,
    bridge: false,
  };
  assert_eq!(run(config, shutdown).await.unwrap(), Exit::Shutdown);
  server.abort();

  let files = live_files(root.path()).unwrap();
  assert_eq!(files.len(), 1);
  let bodies: Vec<_> = Reader::open(&files[0], 0)
    .unwrap()
    .map(|entry| {
      let Record::V0(record) = entry.unwrap().record else {
        panic!("not dgg's")
      };
      assert_eq!(record.instance, "t");
      (record.connection, record.body)
    })
    .collect();
  let [
    (1, Body::Open(url1)),
    (1, Body::Frame(frame1)),
    (1, Body::Binary(binary)),
    (1, Body::Close(_)),
    (2, Body::Open(url2)),
    (2, Body::Frame(frame2)),
    (2, Body::Close(why)),
  ] = &bodies[..]
  else {
    panic!("unexpected journal: {bodies:#?}");
  };
  assert_eq!((url1, url2), (&url, &url));
  assert_eq!(
    (frame1.as_str(), frame2.as_str()),
    ("MSG {\"n\":1}", "MSG {\"n\":2}")
  );
  assert_eq!(binary, &[7]);
  assert_eq!(why, SHUTDOWN);
}

/// Reads every v0 record body from `instance`'s live files in journal
/// `root`.
fn bodies(root: &Path, instance: &str) -> Vec<Body> {
  live_files(root)
    .unwrap()
    .iter()
    .filter(|file| file.file_stem().unwrap() == instance)
    .flat_map(|file| Reader::open(file, 0).unwrap())
    .map(|entry| {
      let Record::V0(record) = entry.unwrap().record else {
        panic!("not dgg's")
      };
      record.body
    })
    .collect()
}

/// An older instance retires once a newer one has been healthy for the
/// grace period, and both leave the control directory clean.
#[::tokio::test]
async fn hands_off_to_a_newer_instance() {
  let (url, server) = chat_server().await;

  let root = TempDir::new("logger-capture");
  let config = |instance: &str, started| Config {
    chat: Options {
      url: url.clone(),
      origin: None,
      ..Options::default()
    },
    journal_root: root.to_path_buf(),
    instance: instance.into(),
    started,
    handoff: quick_handoff(),
    peer: None,
    old_control: None,
    bridge: false,
  };

  let old = ::tokio::spawn(run(config("old", 1), ::std::future::pending()));
  ::tokio::time::sleep(Duration::from_millis(300)).await;
  let (stop, stopped) = oneshot::channel::<()>();
  let new = ::tokio::spawn(run(config("new", 2), async {
    let _ = stopped.await;
  }));

  let old_exit = ::tokio::time::timeout(Duration::from_secs(5), old)
    .await
    .expect("old instance didn't retire")
    .unwrap()
    .unwrap();
  assert_eq!(old_exit, Exit::HandedOff("new".into()));
  stop.send(()).unwrap();
  assert_eq!(new.await.unwrap().unwrap(), Exit::Shutdown);
  server.abort();

  let old = bodies(root.path(), "old");
  assert_eq!(old.last(), Some(&Body::Close(format!("{HANDOFF}new"))));
  let new = bodies(root.path(), "new");
  assert!(matches!(new.first(), Some(Body::Open(_))));
  assert_eq!(new.last(), Some(&Body::Close(SHUTDOWN.into())));
  let control = control::dir(root.path());
  assert_eq!(control::read_all(&control).unwrap(), []);
}

/// A capture from before dgg's journals were by channel, writing `dgg/`,
/// hands off to one writing destiny.gg's, `dgg/destiny.gg/`, which writes
/// its status in the old `control/` too while it's there, and not once it's
/// gone.
#[::tokio::test]
async fn hands_off_from_before_channels() {
  let (url, server) = chat_server().await;
  let root = TempDir::new("logger-capture");
  let dir = Dgg::dir(root.path());
  let old_control = control::dir(&dir);
  let config = |journal_root, instance: &str, started, old_control| Config {
    chat: Options {
      url: url.clone(),
      origin: None,
      ..Options::default()
    },
    journal_root,
    instance: instance.into(),
    started,
    handoff: quick_handoff(),
    peer: None,
    old_control,
    bridge: false,
  };

  let old = config(dir.clone(), "old", 1, None);
  let old = ::tokio::spawn(run(old, ::std::future::pending()));
  ::tokio::time::sleep(Duration::from_millis(300)).await;
  let (stop, stopped) = oneshot::channel::<()>();
  let new = config(destiny(root.path()), "new", 2, Some(old_control.clone()));
  let new = ::tokio::spawn(run(new, async {
    let _ = stopped.await;
  }));
  let old_exit = ::tokio::time::timeout(Duration::from_secs(5), old)
    .await
    .expect("old instance didn't retire")
    .unwrap()
    .unwrap();
  assert_eq!(old_exit, Exit::HandedOff("new".into()));
  // Its copy stops once the old directory's gone.
  let copy = control::path(&old_control, "new");
  assert!(copy.exists());
  ::std::fs::remove_dir_all(&old_control).unwrap();
  ::tokio::time::sleep(Duration::from_millis(100)).await;
  assert!(!old_control.exists());
  stop.send(()).unwrap();
  assert_eq!(new.await.unwrap().unwrap(), Exit::Shutdown);
  server.abort();

  assert_eq!(
    bodies(&dir, "old").last(),
    Some(&Body::Close(format!("{HANDOFF}new")))
  );
  assert!(bodies(&dir, "new").is_empty());
  let new = bodies(&destiny(root.path()), "new");
  assert_eq!(new.last(), Some(&Body::Close(SHUTDOWN.into())));
}

/// Cross-host handoff: the old instance's host runs `serve`; the new
/// instance, on another host (here, another data directory), pushes its
/// status there, and the old one retires as if it were local.
#[::tokio::test]
async fn hands_off_to_an_instance_on_another_host() {
  let (url, server) = chat_server().await;
  let (old_host, new_host) = (
    TempDir::new("logger-capture"),
    TempDir::new("logger-capture"),
  );
  let serve = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let serve_url = format!("http://{}", serve.local_addr().unwrap());
  let tokens = ::logger::serve::Tokens {
    read: "reader".into(),
    control: Some("secret".into()),
    query: None,
  };
  let router = ::logger::serve::router(old_host.to_path_buf(), tokens);
  let serving =
    ::tokio::spawn(async move { ::axum::serve(serve, router).await.unwrap() });

  let config = |root: &TempDir, instance: &str, started, peer| Config {
    chat: Options {
      url: url.clone(),
      origin: None,
      ..Options::default()
    },
    journal_root: destiny(root.path()),
    instance: instance.into(),
    started,
    handoff: quick_handoff(),
    peer,
    old_control: None,
    bridge: false,
  };
  let old = config(&old_host, "old", 1, None);
  let old = ::tokio::spawn(run(old, ::std::future::pending()));
  ::tokio::time::sleep(Duration::from_millis(300)).await;
  let peer = Peer {
    url: serve_url,
    token: "secret".into(),
    ca: None,
  };
  let (stop, stopped) = oneshot::channel::<()>();
  let new = config(&new_host, "new", 2, Some(peer));
  let new = ::tokio::spawn(run(new, async {
    let _ = stopped.await;
  }));

  let old_exit = ::tokio::time::timeout(Duration::from_secs(5), old)
    .await
    .expect("old instance didn't retire")
    .unwrap()
    .unwrap();
  assert_eq!(old_exit, Exit::HandedOff("new".into()));
  stop.send(()).unwrap();
  assert_eq!(new.await.unwrap().unwrap(), Exit::Shutdown);
  server.abort();
  serving.abort();

  let old_bodies = bodies(&destiny(old_host.path()), "old");
  let handoff = Body::Close(format!("{HANDOFF}new"));
  assert_eq!(old_bodies.last(), Some(&handoff));
  // The new instance withdrew its status from the old host on exit.
  let control = control::dir(&destiny(old_host.path()));
  assert_eq!(control::read_all(&control).unwrap(), []);
}
