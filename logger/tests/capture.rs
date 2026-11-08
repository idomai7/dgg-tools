use {
  ::dgg_chat::Options,
  ::futures_util::{SinkExt, StreamExt},
  ::journal::{
    Reader, Record,
    layout::live_files,
    record::v1::{self, Body},
  },
  ::logger::capture::{SHUTDOWN, instance_id, run},
  ::std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
  },
  ::tokio::{net::TcpListener, sync::oneshot},
  ::tokio_tungstenite::tungstenite::Message,
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("logger-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

#[test]
fn instance_ids() {
  assert_eq!(instance_id("deskpc", 5), "deskpc-5");
  assert_eq!(instance_id("my host.lan", 5), "my-host.lan-5");
  assert_eq!(instance_id("", 5), "host-5");
  assert!(v1::valid_instance(&instance_id("π/é:x", 5)));
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

  let root = TempRoot::new();
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
  run(chat, root.0.clone(), "t".into(), shutdown)
    .await
    .unwrap();
  server.abort();

  let files = live_files(&root.0).unwrap();
  assert_eq!(files.len(), 1);
  let bodies: Vec<_> = Reader::open(&files[0], 0)
    .unwrap()
    .map(|e| {
      let Record::V1(r) = e.unwrap().record;
      assert_eq!(r.instance, "t");
      (r.connection, r.body)
    })
    .collect();
  let [
    (1, Body::Open(u1)),
    (1, Body::Frame(f1)),
    (1, Body::Binary(b)),
    (1, Body::Close(_)),
    (2, Body::Open(u2)),
    (2, Body::Frame(f2)),
    (2, Body::Close(why)),
  ] = &bodies[..]
  else {
    panic!("unexpected journal: {bodies:#?}");
  };
  assert_eq!((u1, u2), (&url, &url));
  assert_eq!(
    (f1.as_str(), f2.as_str()),
    ("MSG {\"n\":1}", "MSG {\"n\":2}")
  );
  assert_eq!(b, &[7]);
  assert_eq!(why, SHUTDOWN);
}
