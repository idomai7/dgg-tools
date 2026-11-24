//! Live chat from the logger's `serve`, faked: what the page is told.

mod fake;

use {
  ::common::time::{now_ms, rfc3339},
  ::kick_mod::{
    channels::Channels,
    chat::{Chat, Update},
  },
  ::serde_json::{Value, json},
  ::std::{sync::Arc, time::Duration},
  ::tokio::{sync::broadcast, time::timeout},
  fake::{Kick, QUERY_TOKEN, kick_event, sent},
};

fn channels() -> Channels {
  let channels = json!({"one": {"broadcaster_user_id": 1}});
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
fn follow(logger: &Kick, token: &str) -> (Chat, Updates) {
  let chat = Chat::start(&channels(), &logger.logger(token));
  let (now, later) = chat.watch();
  let now: Vec<_> = now
    .iter()
    .map(|update| ::serde_json::to_value(&**update).unwrap())
    .collect();
  let connecting = json!({"slug": "one", "kind": "status", "state": "connecting", "detail": null});
  assert_eq!(now, [connecting]);
  (chat, Updates(later))
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
  let logger = Kick::start().await;
  let (_chat, mut updates) = follow(&logger, QUERY_TOKEN);
  assert_eq!(updates.status().await, ("live".into(), Value::Null));
  // Already live: an open changes nothing.
  let open = sent(json!({"endpoint": "wss://k"}));
  logger.send("one", "open", open.clone()).await;
  let at = now_ms();
  let event = kick_event("ChatMessageEvent", message("m1", at));
  logger.send("one", "event", event).await;
  let update = updates.next().await;
  assert_eq!(update["kind"], "message");
  assert_eq!(update["id"], "m1");
  assert_eq!(update["content"], "hello from m1");
  let deleted = json!({"id": "d1", "message": {"id": "m1"}});
  let event = kick_event("MessageDeletedEvent", deleted);
  logger.send("one", "event", event).await;
  assert_eq!(
    updates.next().await,
    json!({"slug": "one", "kind": "deleted", "id": "m1", "ai": false})
  );

  let close = sent(json!({"reason": "idle timeout"}));
  logger.send("one", "close", close).await;
  let detail = json!("the capture: idle timeout");
  assert_eq!(updates.status().await, ("reconnecting".into(), detail));
  logger.send("one", "open", open).await;
  assert_eq!(updates.status().await, ("live".into(), Value::Null));
}

#[::tokio::test]
async fn fills_with_unseen_messages_since_it_started() {
  let logger = Kick::start().await;
  let (_chat, mut updates) = follow(&logger, QUERY_TOKEN);
  updates.status().await;
  let at = now_ms();
  let event = kick_event("ChatMessageEvent", message("m1", at));
  logger.send("one", "event", event).await;
  assert_eq!(updates.next().await["id"], "m1");

  // m1 was seen, and m0 is from before following started.
  let page = [
    message("m3", at + 2000),
    message("m2", at + 1000),
    message("m1", at),
    message("m0", at - 3_600_000),
  ];
  logger.send("one", "history", history(&page)).await;
  let filled = updates.next().await;
  assert_eq!(filled["kind"], "filled");
  let ids: Vec<_> = filled["messages"]
    .as_array()
    .unwrap()
    .iter()
    .map(|message| message["id"].as_str().unwrap())
    .collect();
  assert_eq!(ids, ["m2", "m3"]);

  // Nothing new: the page isn't told.
  logger.send("one", "history", history(&page[..2])).await;
  let event = kick_event("ChatMessageEvent", message("m4", at + 3000));
  logger.send("one", "event", event).await;
  assert_eq!(updates.next().await["id"], "m4");
}

#[::tokio::test]
async fn retries_a_dropped_stream() {
  let logger = Kick::start().await;
  let (_chat, mut updates) = follow(&logger, QUERY_TOKEN);
  updates.status().await;
  let open = sent(json!({"endpoint": "wss://k"}));
  logger.send("one", "open", open).await;
  logger.drop_stream("one");
  let (state, detail) = updates.status().await;
  assert_eq!(state, "reconnecting");
  let detail = detail.as_str().unwrap();
  assert!(
    detail.starts_with("the logger: it ended; chat meanwhile is missing;"),
    "{detail}"
  );
  assert_eq!(updates.status().await, ("live".into(), Value::Null));

  logger.send("one", "error", json!("the journal went")).await;
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
  let logger = Kick::start().await;
  let (_chat, mut updates) = follow(&logger, "made-up-wrong-token");
  let detail = json!("the logger: answered 401 Unauthorized");
  assert_eq!(updates.status().await, ("stopped".into(), detail));
}
