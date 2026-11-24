//! `serve`'s Kick endpoints: a user's history, and live chat. Ids and names
//! are made up.

use {
  ::common::temp_dir::TempDir,
  ::logger::{
    kick::index::{FILE, index, open},
    serve::{Tokens, router},
  },
  ::logger_journal::{
    layout::{channel_root, kick_root},
    record::kick::v0::Body,
    writer::kick::Writer,
  },
  ::serde_json::{Value, json},
  ::std::{
    path::PathBuf,
    time::{Duration, Instant},
  },
};

const QUERY: &str = "querier";
const READ: &str = "reader";

fn journal(dir: &TempDir) -> PathBuf {
  dir.path().join("journal")
}

/// Serves `dir`'s journal on a free port; returns its URL.
async fn serve(dir: &TempDir) -> String {
  let listener = ::tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let tokens = Tokens {
    read: READ.into(),
    control: None,
    query: Some(QUERY.into()),
  };
  let app = router(journal(dir), tokens);
  ::tokio::spawn(async move { ::axum::serve(listener, app).await.unwrap() });
  base
}

/// A frame pushing Kick's event `event` (without its `App\Events\`) with
/// `data`.
fn push(event: &str, data: Value) -> Body {
  let frame = json!({
    "push": {
      "channel": "chatrooms.1.v2",
      "pub": {
        "data": {"event": format!("App\\Events\\{event}"), "data": data.to_string()}
      }
    }
  });
  Body::Frame(frame.to_string())
}

fn message(id: &str, user_id: u64, name: &str, content: &str) -> Body {
  push(
    "ChatMessageEvent",
    json!({
      "id": id,
      "content": content,
      "created_at": "2026-11-24T12:00:00+00:00",
      "sender": {"id": user_id, "username": name},
    }),
  )
}

/// Writes `records`, each `(receive time, body)`, as `instance` in channel
/// `c`'s journal.
fn write(dir: &TempDir, instance: &str, records: Vec<(i64, Body)>) {
  let root = channel_root(&kick_root(&journal(dir)), "c");
  let mut writer = Writer::new(root, instance, "c");
  for (time, body) in records {
    writer.write(time, 1, body).unwrap();
  }
}

async fn get(url: &str, token: &str) -> ::reqwest::Response {
  ::reqwest::Client::new()
    .get(url)
    .bearer_auth(token)
    .send()
    .await
    .unwrap()
}

#[::tokio::test]
async fn a_users_history() {
  let dir = TempDir::new("logger-kick-serve");
  write(
    &dir,
    "k",
    vec![
      (10, message("m1", 7, "Seven", "first")),
      (11, message("m2", 8, "Eight", "someone else")),
      (
        12,
        push(
          "UserBannedEvent",
          json!({
            "id": "b1",
            "user": {"id": 7, "username": "Seven"},
            "permanent": false,
            "duration": 1,
          }),
        ),
      ),
    ],
  );
  let mut db = open(&dir.path().join(FILE)).unwrap();
  index(&mut db, &journal(&dir)).unwrap();
  drop(db);
  // Not indexed yet: read from the live file.
  write(
    &dir,
    "k",
    vec![
      (20, message("m3", 7, "Seven2", "renamed")),
      (
        21,
        push("MessageDeletedEvent", json!({"message": {"id": "m3"}})),
      ),
    ],
  );
  let base = serve(&dir).await;

  let history: Value = get(&format!("{base}/kick/channels/c/users/7"), QUERY)
    .await
    .json()
    .await
    .unwrap();
  let ids: Vec<&str> = history["messages"]
    .as_array()
    .unwrap()
    .iter()
    .map(|said| said["id"].as_str().unwrap())
    .collect();
  assert_eq!(ids, ["m1", "m3"]);
  assert_eq!(history["username"], "Seven2");
  assert_eq!(history["messages"][0]["deleted"], Value::Null);
  assert_eq!(
    history["messages"][1]["deleted"],
    json!({"received": 21, "ai": false})
  );
  assert_eq!(history["events"][0]["kind"], "timeout");
  assert_eq!(history["events"][0]["minutes"], 1);
  assert_eq!(history["earlier"], 0);

  let code = |path: &'static str, token: &'static str| {
    let url = format!("{base}{path}");
    async move { get(&url, token).await.status().as_u16() }
  };
  // The read token reads it too; the query token reads nothing else.
  assert_eq!(code("/kick/channels/c/users/7", READ).await, 200);
  assert_eq!(code("/kick/manifest", QUERY).await, 403);
  assert_eq!(code("/dgg/manifest", QUERY).await, 403);
  assert_eq!(
    code("/kick/files/c/live/1970-01-01/k.journal", QUERY).await,
    403
  );
  assert_eq!(code("/kick/channels/c/users/7", "nobody").await, 401);
  assert_eq!(code("/kick/channels/d/users/7", QUERY).await, 404);
  assert_eq!(code("/kick/channels/C/users/7", QUERY).await, 400);
  assert_eq!(code("/kick/channels/c/users/x", QUERY).await, 400);
}

/// Reads server-sent events from `response` until `done` says it has what
/// it wants, or 5 seconds pass; each `(event, data)`.
async fn events(
  response: &mut ::reqwest::Response,
  done: impl Fn(&[(String, Value)]) -> bool,
) -> Vec<(String, Value)> {
  let deadline = Instant::now() + Duration::from_secs(5);
  let (mut text, mut found) = (String::new(), Vec::new());
  while !done(&found) {
    let left = deadline.saturating_duration_since(Instant::now());
    let chunk = ::tokio::time::timeout(left, response.chunk())
      .await
      .expect("no more events in time")
      .unwrap()
      .unwrap();
    text.push_str(&String::from_utf8_lossy(&chunk));
    while let Some(end) = text.find("\n\n") {
      let block: String = text.drain(..end + 2).collect();
      let (mut event, mut data) = (None, None);
      for line in block.lines() {
        if let Some(name) = line.strip_prefix("event: ") {
          event = Some(name.to_owned());
        }
        if let Some(json) = line.strip_prefix("data: ") {
          data = Some(::serde_json::from_str(json).unwrap());
        }
      }
      if let (Some(event), Some(data)) = (event, data) {
        found.push((event, data));
      }
    }
  }
  found
}

#[::tokio::test]
async fn live_chat_from_now_on() {
  let dir = TempDir::new("logger-kick-serve");
  write(&dir, "k", vec![(10, message("m0", 7, "Seven", "before"))]);
  let base = serve(&dir).await;
  let mut response =
    get(&format!("{base}/kick/channels/c/events"), QUERY).await;
  assert_eq!(response.status().as_u16(), 200);

  // Two captures overlapping: each message is passed on once.
  write(
    &dir,
    "k",
    vec![
      (20, message("m1", 7, "Seven", "hi")),
      (21, push("ChatroomClearEvent", json!({"id": "x"}))),
    ],
  );
  write(
    &dir,
    "l",
    vec![
      (19, Body::Open("wss://one".into())),
      (20, message("m1", 7, "Seven", "hi")),
      (22, message("m2", 7, "Seven", "again")),
    ],
  );
  let found = events(&mut response, |found| found.len() >= 4).await;
  let kinds: Vec<&str> = found.iter().map(|(kind, _)| kind.as_str()).collect();
  assert_eq!(kinds.iter().filter(|kind| **kind == "open").count(), 1);
  let messages: Vec<String> = found
    .iter()
    .filter(|(kind, _)| kind == "event")
    .filter(|(_, data)| data["event"] == "App\\Events\\ChatMessageEvent")
    .map(|(_, data)| {
      let message: Value =
        ::serde_json::from_str(data["data"].as_str().unwrap()).unwrap();
      message["id"].as_str().unwrap().to_owned()
    })
    .collect();
  let mut sorted = messages.clone();
  sorted.sort();
  assert_eq!(sorted, ["m1", "m2"], "{found:?}");
  let (_, open) = found.iter().find(|(kind, _)| kind == "open").unwrap();
  assert_eq!(open["endpoint"], "wss://one");
  assert_eq!(
    (open["instance"].as_str(), open["received"].as_i64()),
    (Some("l"), Some(19))
  );
}
