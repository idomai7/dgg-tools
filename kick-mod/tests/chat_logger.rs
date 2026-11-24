//! Live chat from the logger's `serve`, faked: what the page is told.

use {
  ::axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
  },
  ::common::{
    config::Secret,
    time::{now_ms, rfc3339},
  },
  ::futures_util::stream,
  ::kick_mod::{
    channels::Channels,
    chat::{Chat, Update},
    config, logger,
  },
  ::serde_json::{Value, json},
  ::std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
  },
  ::tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    time::{sleep, timeout},
  },
};

const TOKEN: &str = "made-up-query-token";

/// The logger's `serve`, with one channel's stream at a time, fed by the
/// test.
struct Logger {
  url: String,
  stream: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
}

impl Logger {
  async fn start() -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stream = Arc::new(Mutex::new(None));
    let router = Router::new()
      .route("/kick/v0/channels/{slug}/events", get(events))
      .with_state(stream.clone());
    ::tokio::spawn(async move { ::axum::serve(listener, router).await });
    Self { url, stream }
  }

  fn client(&self, token: &str) -> logger::Client {
    let logger = config::Logger {
      url: format!("{}/", self.url),
      token: Secret::new(token),
      ca: None,
    };
    let client = logger::Client::new(&logger).unwrap();
    client.with_first_retry(Duration::from_millis(10))
  }

  /// Sends event `kind` with `data`, once a stream is open.
  async fn send(&self, kind: &str, data: Value) {
    let event = format!("event: {kind}\ndata: {data}\n\n");
    for _ in 0..500 {
      if let Some(stream) = &*self.stream.lock().unwrap() {
        stream.send(event).unwrap();
        return;
      }
      sleep(Duration::from_millis(10)).await;
    }
    panic!("no stream opened");
  }

  /// Ends the stream that's open.
  fn drop_stream(&self) {
    self.stream.lock().unwrap().take().unwrap();
  }
}

async fn events(
  State(stream): State<Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>>,
  Path(slug): Path<String>,
  headers: HeaderMap,
) -> Response {
  let bearer = format!("Bearer {TOKEN}");
  if headers
    .get(header::AUTHORIZATION)
    .unwrap()
    .to_str()
    .unwrap()
    != bearer
  {
    return StatusCode::UNAUTHORIZED.into_response();
  }
  assert_eq!(slug, "one");
  let (sender, receiver) = mpsc::unbounded_channel::<String>();
  *stream.lock().unwrap() = Some(sender);
  let body = stream::unfold(receiver, |mut receiver| async move {
    let text = receiver.recv().await?;
    Some((Ok::<_, Infallible>(text), receiver))
  });
  (
    [(header::CONTENT_TYPE, "text/event-stream")],
    Body::from_stream(body),
  )
    .into_response()
}

fn channels() -> Channels {
  let channels =
    json!({"one": {"broadcaster_user_id": 1, "chatroom_id": null}});
  ::serde_json::from_value(channels).unwrap()
}

/// The page's updates, as JSON.
struct Updates(broadcast::Receiver<Arc<Update>>);

impl Updates {
  async fn next(&mut self) -> Value {
    let update = timeout(Duration::from_secs(5), self.0.recv());
    let update = update.await.expect("no update").unwrap();
    ::serde_json::to_value(&*update).unwrap()
  }

  async fn status(&mut self) -> (String, Value) {
    let update = self.next().await;
    assert_eq!(update["kind"], "status", "{update}");
    (
      update["state"].as_str().unwrap().into(),
      update["detail"].clone(),
    )
  }
}

/// Follows channel `one`, past its first status: connecting.
fn follow(logger: &Logger, token: &str) -> (Chat, Updates) {
  let chat = Chat::from_logger(&channels(), &logger.client(token));
  let (now, later) = chat.watch();
  let now: Vec<_> = now
    .iter()
    .map(|update| ::serde_json::to_value(&**update).unwrap())
    .collect();
  let connecting = json!({"slug": "one", "kind": "status", "state": "connecting", "detail": null});
  assert_eq!(now, [connecting]);
  (chat, Updates(later))
}

fn sent(more: Value) -> Value {
  let mut sent =
    json!({"received": now_ms(), "instance": "capture-1", "connection": 1});
  sent
    .as_object_mut()
    .unwrap()
    .extend(more.as_object().unwrap().clone());
  sent
}

fn message(id: &str, at: i64) -> Value {
  json!({
    "id": id,
    "chatroom_id": 1000,
    "content": format!("hello from {id}"),
    "type": "message",
    "created_at": rfc3339(at),
    "sender": {"id": 7, "username": "someone", "slug": "someone"},
  })
}

/// Kick's event `name` with `data`, as the logger passes it on.
fn kick_event(name: &str, data: Value) -> Value {
  let event = format!("App\\Events\\{name}");
  sent(json!({"event": event, "data": data.to_string()}))
}

/// A history page of `messages`, newest first, as the logger passes it on.
fn history(messages: &[Value]) -> Value {
  let page = json!({
    "data": {"messages": messages, "cursor": "1", "pinned_message": null},
    "message": "OK",
  });
  sent(json!({"page": page.to_string()}))
}

#[::tokio::test]
async fn follows_chat_and_the_capture() {
  let logger = Logger::start().await;
  let (_chat, mut updates) = follow(&logger, TOKEN);
  assert_eq!(updates.status().await, ("live".into(), Value::Null));
  // Already live: an open changes nothing.
  logger
    .send("open", sent(json!({"endpoint": "wss://k"})))
    .await;
  let at = now_ms();
  let event = kick_event("ChatMessageEvent", message("m1", at));
  logger.send("event", event).await;
  let update = updates.next().await;
  assert_eq!(update["kind"], "message");
  assert_eq!(update["id"], "m1");
  assert_eq!(update["content"], "hello from m1");
  let deleted = json!({"id": "d1", "message": {"id": "m1"}});
  logger
    .send("event", kick_event("MessageDeletedEvent", deleted))
    .await;
  assert_eq!(
    updates.next().await,
    json!({"slug": "one", "kind": "deleted", "id": "m1", "ai": false})
  );

  logger
    .send("close", sent(json!({"reason": "idle timeout"})))
    .await;
  let detail = json!("the capture: idle timeout");
  assert_eq!(updates.status().await, ("reconnecting".into(), detail));
  logger
    .send("open", sent(json!({"endpoint": "wss://k"})))
    .await;
  assert_eq!(updates.status().await, ("live".into(), Value::Null));
}

#[::tokio::test]
async fn fills_with_unseen_messages_since_it_started() {
  let logger = Logger::start().await;
  let (_chat, mut updates) = follow(&logger, TOKEN);
  updates.status().await;
  let at = now_ms();
  let event = kick_event("ChatMessageEvent", message("m1", at));
  logger.send("event", event).await;
  assert_eq!(updates.next().await["id"], "m1");

  // m1 was seen, and m0 is from before following started.
  let page = [
    message("m3", at + 2000),
    message("m2", at + 1000),
    message("m1", at),
    message("m0", at - 3_600_000),
  ];
  logger.send("history", history(&page)).await;
  let filled = updates.next().await;
  assert_eq!(filled["kind"], "filled");
  assert_eq!(filled["partial"], false);
  let ids: Vec<_> = filled["messages"]
    .as_array()
    .unwrap()
    .iter()
    .map(|message| message["id"].as_str().unwrap())
    .collect();
  assert_eq!(ids, ["m2", "m3"]);

  // Nothing new: the page isn't told.
  logger.send("history", history(&page[..2])).await;
  let event = kick_event("ChatMessageEvent", message("m4", at + 3000));
  logger.send("event", event).await;
  assert_eq!(updates.next().await["id"], "m4");
}

#[::tokio::test]
async fn retries_a_dropped_stream() {
  let logger = Logger::start().await;
  let (_chat, mut updates) = follow(&logger, TOKEN);
  updates.status().await;
  logger
    .send("open", sent(json!({"endpoint": "wss://k"})))
    .await;
  logger.drop_stream();
  let (state, detail) = updates.status().await;
  assert_eq!(state, "reconnecting");
  let detail = detail.as_str().unwrap();
  assert!(
    detail.starts_with("the logger: it ended; chat meanwhile is missing;"),
    "{detail}"
  );
  assert_eq!(updates.status().await, ("live".into(), Value::Null));

  logger.send("error", json!("the journal went")).await;
  let (state, detail) = updates.status().await;
  assert_eq!(state, "reconnecting");
  let detail = detail.as_str().unwrap();
  assert!(
    detail.starts_with("the logger: its error: \"the journal went\";"),
    "{detail}"
  );
  assert_eq!(updates.status().await, ("live".into(), Value::Null));
}

#[::tokio::test]
async fn stops_when_refused() {
  let logger = Logger::start().await;
  let (_chat, mut updates) = follow(&logger, "made-up-wrong-token");
  let detail = json!("the logger: answered 401 Unauthorized");
  assert_eq!(updates.status().await, ("stopped".into(), detail));
}
