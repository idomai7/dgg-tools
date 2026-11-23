//! Kick capture, against local servers standing in for Kick's Centrifugo
//! and chat history. Slugs, ids and messages are made up.

use {
  ::common::{temp_dir::TempDir, time::now_ms},
  ::futures_util::{SinkExt, StreamExt},
  ::kick_api::{
    realtime::{Options, Setup},
    web::Web,
  },
  ::logger::{
    capture::{Exit, HANDOFF, Handoff, SHUTDOWN},
    control::{self, Status},
    kick::{
      capture::{Config, control_dir, run},
      channels::Channel,
    },
  },
  ::logger_journal::{
    Reader, Record, Variant,
    layout::{channel_root, live_files},
    record::kick::v0::{self, Body},
  },
  ::std::{
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
  },
  ::tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::{sleep, timeout},
  },
  ::tokio_tungstenite::{WebSocketStream, tungstenite::Message},
};

type Ws = WebSocketStream<TcpStream>;

/// Kick's Centrifugo, taking connections in turn.
struct Centrifugo {
  url: String,
  listener: TcpListener,
}

impl Centrifugo {
  async fn start() -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    Self { url, listener }
  }

  fn options(&self) -> Options {
    Options {
      setup: Setup::Fixed {
        endpoint: self.url.clone(),
        token: "t0k3n".into(),
      },
      min_backoff: Duration::from_millis(10),
      max_backoff: Duration::from_millis(50),
      ..Options::default()
    }
  }

  /// Accepts a connection, answers its connect and its subscribe: the
  /// socket, and the channel subscribed to.
  async fn accept(&self) -> (Ws, String) {
    let (tcp, _) = within(self.listener.accept()).await.unwrap();
    let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
    assert!(receive(&mut ws).await.contains(r#""connect""#));
    send(&mut ws, r#"{"id":1,"connect":{"client":"c1","ping":25}}"#).await;
    let subscribe: ::serde_json::Value =
      ::serde_json::from_str(&receive(&mut ws).await).unwrap();
    let channel = subscribe["subscribe"]["channel"].as_str().unwrap();
    send(&mut ws, r#"{"id":2,"subscribe":{}}"#).await;
    (ws, channel.to_owned())
  }
}

async fn send(ws: &mut Ws, text: &str) {
  ws.send(Message::text(text)).await.unwrap();
}

async fn receive(ws: &mut Ws) -> String {
  match within(ws.next()).await {
    Some(Ok(Message::Text(text))) => text.as_str().to_owned(),
    other => panic!("expected text, got {other:?}"),
  }
}

/// A publication on `channel`, as Centrifugo frames it.
fn push(channel: &str, text: &str) -> String {
  ::serde_json::json!({
    "push": {"channel": channel, "pub": {"data": {"said": text}}},
  })
  .to_string()
}

/// Kick's chat history: every page `page`, and the paths asked.
async fn history(page: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let asked = Arc::new(Mutex::new(Vec::new()));
  let kept = asked.clone();
  let router = ::axum::Router::new().fallback(move |uri: ::axum::http::Uri| {
    kept.lock().unwrap().push(uri.to_string());
    async move { page }
  });
  ::tokio::spawn(async move { ::axum::serve(listener, router).await });
  (base, asked)
}

const PAGE: &str =
  r#"{"data":{"messages":[],"cursor":"","pinned_message":null}}"#;

async fn within<T>(future: impl Future<Output = T>) -> T {
  timeout(Duration::from_secs(5), future)
    .await
    .expect("in time")
}

/// Waits for `ready`.
async fn until(mut ready: impl FnMut() -> bool) {
  within(async {
    while !ready() {
      sleep(Duration::from_millis(10)).await;
    }
  })
  .await
}

fn quick_handoff() -> Handoff {
  Handoff {
    grace: Duration::from_millis(200),
    stale_after: Duration::from_secs(2),
    interval: Duration::from_millis(20),
    keep_running: false,
  }
}

fn channel(slug: &str, id: u64) -> Channel {
  Channel {
    slug: slug.into(),
    channel_id: id + 1,
    chatroom_id: id,
  }
}

struct Running {
  dir: TempDir,
  stop: oneshot::Sender<()>,
  task: JoinHandle<::std::io::Result<Exit>>,
}

fn start(
  centrifugo: &Centrifugo,
  channels: Vec<Channel>,
  web: Option<Web>,
) -> Running {
  let dir = TempDir::new("logger-kick-capture");
  let config = Config {
    channels,
    realtime: centrifugo.options(),
    web: web.map(Arc::new),
    kick_root: dir.path().to_owned(),
    instance: "here-5".into(),
    started: 5,
    handoff: quick_handoff(),
    restart_after: Duration::from_millis(10),
    bridge: false,
  };
  let (stop, stopped) = oneshot::channel();
  let shutdown = async move {
    let _ = stopped.await;
  };
  let task = ::tokio::spawn(run(config, shutdown));
  Running { dir, stop, task }
}

/// The records journaled for `slug`, in order.
fn records(root: &Path, slug: &str) -> Vec<v0::Record> {
  live_files(&channel_root(root, slug))
    .unwrap()
    .iter()
    .flat_map(|file| Reader::open(file, 0).unwrap())
    .map(|entry| match entry.unwrap().record {
      Record::Kick0(record) => record,
      record => panic!("not Kick's: {record:?}"),
    })
    .collect()
}

fn bodies(root: &Path, slug: &str) -> Vec<(u64, Body)> {
  records(root, slug)
    .into_iter()
    .map(|record| (record.connection, record.body))
    .collect()
}

fn statuses(root: &Path, slug: &str) -> Vec<Status> {
  control::read_all_as(&control_dir(root, slug), Variant::Kick).unwrap()
}

#[tokio::test]
async fn captures_each_channel_apart() {
  let centrifugo = Centrifugo::start().await;
  let channels = vec![channel("one", 1000), channel("two", 2000)];
  let running = start(&centrifugo, channels, None);
  let root = running.dir.path().to_owned();
  let mut sockets = Vec::new();
  for _ in 0..2 {
    let (mut ws, room) = centrifugo.accept().await;
    send(&mut ws, &push(&room, &format!("hi {room}"))).await;
    sockets.push(ws);
  }
  // Healthy once both are subscribed.
  until(|| {
    ["one", "two"].iter().all(|slug| {
      statuses(&root, slug)
        .first()
        .is_some_and(|status| status.healthy_since.is_some())
    })
  })
  .await;
  until(|| {
    ["one", "two"]
      .iter()
      .all(|slug| bodies(&root, slug).len() >= 4)
  })
  .await;

  running.stop.send(()).unwrap();
  let exit = within(running.task).await.unwrap().unwrap();
  assert_eq!(exit, Exit::Shutdown);
  for (slug, room) in [("one", 1000), ("two", 2000)] {
    let bodies = bodies(&root, slug);
    let [
      (1, Body::Open(url)),
      (1, Body::Frame(_connected)),
      (1, Body::Frame(_subscribed)),
      (1, Body::Frame(said)),
      (1, Body::Close(reason)),
    ] = &bodies[..]
    else {
      panic!("{slug}: {bodies:?}");
    };
    assert_eq!(url, &centrifugo.url);
    let room = format!("chatrooms.{room}.v2");
    assert!(said.contains(&format!("hi {room}")), "{said}");
    assert_eq!(reason, SHUTDOWN);
    assert_eq!(statuses(&root, slug), []);
  }
  // Each file names its channel.
  let file = &live_files(&channel_root(&root, "one")).unwrap()[0];
  let text = ::std::fs::read_to_string(file).unwrap();
  assert!(text.starts_with("journal kick.0.1 ; "), "{text}");
  assert!(
    text.lines().next().unwrap().contains(" channel=one"),
    "{text}"
  );
}

#[tokio::test]
async fn fills_a_gap_from_the_history() {
  let centrifugo = Centrifugo::start().await;
  let (base, asked) = history(PAGE).await;
  let web =
    Web::new(::reqwest::Client::new(), "ledgerline-tests/0").with_base(base);
  let running = start(&centrifugo, vec![channel("one", 1000)], Some(web));
  let root = running.dir.path().to_owned();
  let (mut ws, room) = centrifugo.accept().await;
  send(&mut ws, &push(&room, "before")).await;
  until(|| bodies(&root, "one").len() >= 4).await;
  // The first subscribe isn't a gap.
  assert!(asked.lock().unwrap().is_empty());

  drop(ws);
  let _ws = centrifugo.accept().await;
  until(|| {
    bodies(&root, "one")
      .iter()
      .any(|(_, body)| matches!(body, Body::History { .. }))
  })
  .await;
  assert_eq!(*asked.lock().unwrap(), ["/chat/1001/history"]);
  let history: Vec<_> = bodies(&root, "one")
    .into_iter()
    .filter(|(_, body)| matches!(body, Body::History { .. }))
    .collect();
  let page = PAGE.to_owned();
  assert_eq!(history, [(2, Body::History { cursor: None, page })]);
  running.stop.send(()).unwrap();
  within(running.task).await.unwrap().unwrap();
}

#[tokio::test]
async fn starts_again_after_kick_stops_it() {
  let centrifugo = Centrifugo::start().await;
  let running = start(&centrifugo, vec![channel("one", 1000)], None);
  let root = running.dir.path().to_owned();
  let (mut ws, _) = centrifugo.accept().await;
  let stop = r#"{"push":{"disconnect":{"code":3501,"reason":"bad request"}}}"#;
  send(&mut ws, stop).await;
  // A new session, its connections numbered on from the last.
  let _ws = centrifugo.accept().await;
  // Open, connected, subscribed, the disconnect and its close; then
  // open, connected and subscribed again.
  until(|| bodies(&root, "one").len() >= 8).await;
  running.stop.send(()).unwrap();
  within(running.task).await.unwrap().unwrap();
  let bodies = bodies(&root, "one");
  let connections: Vec<_> = bodies.iter().map(|(c, _)| *c).collect();
  assert_eq!(connections, [1, 1, 1, 1, 1, 2, 2, 2, 2], "{bodies:?}");
  assert!(matches!(&bodies[4].1, Body::Close(r) if r == "3501 bad request"));
  assert!(matches!(&bodies[8].1, Body::Close(r) if r == SHUTDOWN));
}

#[tokio::test]
async fn hands_off_to_a_newer_instance() {
  let centrifugo = Centrifugo::start().await;
  let channels = vec![channel("one", 1000), channel("two", 2000)];
  let running = start(&centrifugo, channels, None);
  let root = running.dir.path().to_owned();
  let _sockets = (centrifugo.accept().await, centrifugo.accept().await);
  until(|| !statuses(&root, "two").is_empty()).await;

  // A newer instance, healthy a while, capturing only channel two.
  let newer = Status {
    instance: "there-9".into(),
    started: 9,
    heartbeat: now_ms(),
    healthy_since: Some(now_ms() - 60_000),
    build: None,
  };
  let dir = control_dir(&root, "two");
  control::write_as(&dir, &newer, Variant::Kick).unwrap();
  let exit = within(running.task).await.unwrap().unwrap();
  assert_eq!(exit, Exit::HandedOff("there-9".into()));
  for slug in ["one", "two"] {
    let bodies = bodies(&root, slug);
    let handoff = format!("{HANDOFF}there-9");
    assert!(
      matches!(bodies.last(), Some((1, Body::Close(r))) if *r == handoff),
      "{slug}: {bodies:?}"
    );
  }
  // Only its own status is gone.
  assert_eq!(statuses(&root, "one"), []);
  assert_eq!(statuses(&root, "two"), [newer]);
}
