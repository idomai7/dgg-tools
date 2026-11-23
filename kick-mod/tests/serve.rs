//! The page's server, on a port of its own, with local servers standing
//! in for Kick's. Slugs, ids, names and tokens are made up.

mod fake;

use {
  ::common::temp_dir::TempDir,
  ::kick_api::{official::oauth::App, realtime::Options},
  ::kick_mod::{
    account::Account,
    actions::{Action, Asked, Target},
    channels,
    chat::{Chat, Journal},
    modlog::{self, Answer, Entry, Moderator},
    serve::{self, Served},
  },
  ::logger_journal::{
    Reader, Record,
    layout::live_files,
    record::kick::v0::{self, Body},
  },
  ::reqwest::{
    Url,
    header::{HOST, LOCATION, ORIGIN},
    redirect::Policy,
  },
  ::serde_json::{Value, json},
  ::std::{fs, time::Duration},
  ::tokio::{net::TcpListener, time::timeout},
  fake::{Centrifugo, Kick},
};

/// The server, with its files in a directory removed when the test ends.
struct Server {
  base: String,
  port: u16,
  dir: TempDir,
  kick: Kick,
  http: ::reqwest::Client,
}

impl Server {
  async fn start() -> Self {
    Self::start_with("{}", Options::default()).await
  }

  /// With `channels` as `channels.json`, their chat connecting with
  /// `options`.
  async fn start_with(channels: &str, options: Options) -> Self {
    let dir = TempDir::new("kick-mod-serve");
    dir.write("channels.json", channels);
    let kick = Kick::start().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut app = App::new("app-id", "app-secret");
    app.base = kick.base.clone();
    let redirect = format!("http://localhost:{port}/oauth/callback");
    let token = dir.path().join("token.json");
    let http = ::reqwest::Client::new();
    let account = Account::new(http, app, &kick.base, redirect, token);
    let channels = dir.path().join("channels.json");
    let served = Served {
      chat: Chat::start(
        &channels::load(&channels).unwrap(),
        &options,
        &Journal::new(dir.path().join("journal"), 5),
      ),
      channels,
      journal: dir.path().join("journal"),
      modlog: dir.path().join("modlog.jsonl"),
      account: account.unwrap(),
    };
    ::tokio::spawn(serve::run(listener, served));
    Self {
      base: format!("http://127.0.0.1:{port}"),
      port,
      dir,
      kick,
      http: ::reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .unwrap(),
    }
  }

  async fn get(&self, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut request = self.http.get(format!("{}{path}", self.base));
    for &(name, value) in headers {
      request = request.header(name, value);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
  }

  /// Where `path` redirects to.
  async fn redirect(&self, path: &str) -> String {
    let response = self
      .http
      .get(format!("{}{path}", self.base))
      .send()
      .await
      .unwrap();
    assert_eq!(response.status().as_u16(), 303);
    response.headers()[LOCATION].to_str().unwrap().to_owned()
  }

  fn channels(&self) -> ::std::path::PathBuf {
    self.dir.path().join("channels.json")
  }

  /// Signs in, as user 7, `moderator`, with the access token `access-1`.
  async fn sign_in(&self) {
    let to_kick = Url::parse(&self.redirect("/sign-in").await).unwrap();
    let (_, state) = to_kick.query_pairs().find(|(k, _)| k == "state").unwrap();
    self.kick.answer(
      "/oauth/token",
      200,
      r#"{"access_token":"access-1","refresh_token":"refresh-1","expires_in":3600}"#,
    );
    self.kick.answer(
      "/users",
      200,
      r#"{"data":[{"user_id":7,"name":"moderator"}],"message":"OK"}"#,
    );
    let back = format!("/oauth/callback?code=code-1&state={state}");
    assert_eq!(self.redirect(&back).await, "/");
  }

  async fn act(&self, asked: Value) -> (u16, String) {
    let url = format!("{}/api/actions", self.base);
    let response = self.http.post(url).json(&asked).send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
  }

  /// The mod log, so far.
  fn modlog(&self) -> Vec<Entry> {
    let path = self.dir.path().join("modlog.jsonl");
    modlog::parse(&fs::read_to_string(path).unwrap_or_default()).unwrap()
  }

  async fn events(&self) -> Events {
    let url = format!("{}/api/events", self.base);
    let response = self.http.get(url).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    Events {
      response,
      buffer: String::new(),
    }
  }
}

/// Server-sent events, read as they come.
struct Events {
  response: ::reqwest::Response,
  buffer: String,
}

impl Events {
  /// The next event's data, as JSON.
  async fn next(&mut self) -> Value {
    loop {
      if let Some(end) = self.buffer.find("\n\n") {
        let event: String = self.buffer.drain(..end + 2).collect();
        let data: Vec<_> = event
          .lines()
          .filter_map(|line| line.strip_prefix("data: "))
          .collect();
        // A keep-alive has none.
        if !data.is_empty() {
          return ::serde_json::from_str(&data.join("\n")).unwrap();
        }
        continue;
      }
      let chunk = timeout(Duration::from_secs(5), self.response.chunk());
      let chunk = chunk.await.expect("an event in time").unwrap().unwrap();
      self.buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
  }

  /// The state of `slug`'s connection next shown.
  async fn status(&mut self, slug: &str) -> Value {
    loop {
      let event = self.next().await;
      if event["slug"] == slug && event["kind"] == "status" {
        return event["state"].clone();
      }
    }
  }
}

#[tokio::test]
async fn serves_the_page() {
  let server = Server::start().await;
  let (status, page) = server.get("/", &[]).await;
  assert_eq!(status, 200);
  assert!(page.contains("<title>kick-mod</title>"));
}

#[tokio::test]
async fn lists_the_channels_as_they_are_now() {
  let server = Server::start().await;
  assert_eq!(server.get("/api/channels", &[]).await, (200, "{}".into()));
  fs::write(
    server.channels(),
    r#"{"one": {"broadcaster_user_id": 1, "chatroom_id": null}}"#,
  )
  .unwrap();
  let (status, body) = server.get("/api/channels", &[]).await;
  assert_eq!(status, 200);
  assert_eq!(
    ::serde_json::from_str::<::serde_json::Value>(&body).unwrap(),
    ::serde_json::json!({"one": {"broadcaster_user_id": 1, "chatroom_id": null}})
  );
  fs::write(server.channels(), "{ not json").unwrap();
  assert_eq!(server.get("/api/channels", &[]).await.0, 500);
}

#[tokio::test]
async fn only_for_itself() {
  let server = Server::start().await;
  let port = server.port;
  let own = format!("127.0.0.1:{port}");
  let local = format!("localhost:{port}");
  let elsewhere = format!("example.com:{port}");
  for host in [&own, &local, &local.to_uppercase()] {
    assert_eq!(server.get("/", &[(HOST.as_str(), host)]).await.0, 200);
  }
  // A website's name pointed at this address: DNS rebinding.
  assert_eq!(server.get("/", &[(HOST.as_str(), &elsewhere)]).await.0, 403);
  assert_eq!(
    server.get("/", &[(HOST.as_str(), "127.0.0.1")]).await.0,
    403
  );

  for origin in [format!("http://{own}"), format!("http://{local}")] {
    let headers = [(ORIGIN.as_str(), &*origin)];
    assert_eq!(server.get("/api/channels", &headers).await.0, 200);
  }
  // A website's page asking it.
  for origin in [
    "https://example.com",
    &format!("http://{elsewhere}"),
    &format!("https://{own}"),
    "null",
  ] {
    let headers = [(ORIGIN.as_str(), origin)];
    let (status, body) = server.get("/api/channels", &headers).await;
    assert_eq!((status, &*body), (403, "not this server's"));
  }
}

#[tokio::test]
async fn signs_in_and_out() {
  let server = Server::start().await;
  let signed_out = r#"{"user":null}"#;
  assert_eq!(server.get("/api/me", &[]).await, (200, signed_out.into()));

  let to_kick = server.redirect("/sign-in").await;
  assert!(
    to_kick.starts_with(&format!("{}/oauth/authorize?", server.kick.base))
  );
  let asked: Vec<_> = Url::parse(&to_kick)
    .unwrap()
    .query_pairs()
    .into_owned()
    .collect();
  let get = |key: &str| asked.iter().find(|(k, _)| k == key).unwrap().1.clone();
  let port = server.port;
  assert_eq!(
    get("redirect_uri"),
    format!("http://localhost:{port}/oauth/callback")
  );

  server.kick.answer(
    "/oauth/token",
    200,
    r#"{"access_token":"access-1","refresh_token":"refresh-1","expires_in":3600}"#,
  );
  server.kick.answer(
    "/users",
    200,
    r#"{"data":[{"user_id":7,"name":"moderator"}],"message":"OK"}"#,
  );
  let back = format!("/oauth/callback?code=code-1&state={}", get("state"));
  assert_eq!(server.redirect(&back).await, "/");
  let signed_in = r#"{"user":{"name":"moderator","user_id":7}}"#;
  assert_eq!(server.get("/api/me", &[]).await, (200, signed_in.into()));
  assert!(server.dir.path().join("token.json").exists());

  server.kick.answer("/oauth/revoke", 200, "OK");
  let out = server.http.post(format!("{}/api/sign-out", server.base));
  assert_eq!(out.send().await.unwrap().status().as_u16(), 204);
  assert_eq!(server.get("/api/me", &[]).await, (200, signed_out.into()));
  assert!(!server.dir.path().join("token.json").exists());
}

#[tokio::test]
async fn says_why_it_didnt_sign_in() {
  let server = Server::start().await;
  let (status, page) = server
    .get("/oauth/callback?code=c&state=made-up", &[])
    .await;
  assert_eq!(status, 400);
  assert!(page.contains("Not signed in: this sign-in isn't one"));

  // What Kick says is shown as text.
  let to_kick = server.redirect("/sign-in").await;
  let url = Url::parse(&to_kick).unwrap();
  let (_, state) = url.query_pairs().find(|(k, _)| k == "state").unwrap();
  let back = format!("/oauth/callback?error=%3Cb%3Eno%3C/b%3E&state={state}");
  let (status, page) = server.get(&back, &[]).await;
  assert_eq!(status, 400);
  assert!(page.contains("Kick refused: &lt;b&gt;no&lt;/b&gt;"));
}

#[tokio::test]
async fn signing_out_needs_its_own_origin() {
  let server = Server::start().await;
  let out = server
    .http
    .post(format!("{}/api/sign-out", server.base))
    .header(ORIGIN, "https://example.com");
  assert_eq!(out.send().await.unwrap().status().as_u16(), 403);
}

const ROOM: &str = "chatrooms.1000.v2";

#[tokio::test]
async fn streams_chat() {
  let centrifugo = Centrifugo::start().await;
  let channels = r#"{
    "one": {"broadcaster_user_id": 1, "chatroom_id": 1000},
    "two": {"broadcaster_user_id": 2, "chatroom_id": null}
  }"#;
  let server = Server::start_with(channels, centrifugo.options()).await;
  let mut events = server.events().await;
  // Each channel's status first.
  assert_eq!(
    events.next().await,
    json!({"slug": "one", "kind": "status", "state": "connecting", "detail": null})
  );
  assert_eq!(
    events.next().await,
    json!({
      "slug": "two",
      "kind": "status",
      "state": "off",
      "detail": "no chatroom id: enter it in channels.json",
    })
  );
  let mut ws = centrifugo.accept(ROOM).await;
  assert_eq!(events.status("one").await, "live");

  let sender = json!({
    "id": 7,
    "username": "someone",
    "slug": "someone",
    "identity": {
      "color": "#00FF00",
      "badges": [{"type": "moderator", "text": "Moderator"}],
    },
  });
  let message = json!({
    "id": "m1",
    "chatroom_id": 1000,
    "content": "hi [emote:1:wave]",
    "type": "message",
    "created_at": "2026-11-23T10:00:00+00:00",
    "sender": sender,
    "metadata": null,
  });
  fake::publish(&mut ws, ROOM, "ChatMessageEvent", message).await;
  assert_eq!(
    events.next().await,
    json!({
      "slug": "one",
      "kind": "message",
      "id": "m1",
      "at": ::common::time::unix_ms("2026-11-23T10:00:00Z"),
      "sender": {
        "id": 7,
        "username": "someone",
        "color": "#00FF00",
        "badges": ["moderator"],
      },
      "content": "hi [emote:1:wave]",
    })
  );

  let deleted =
    json!({"id": "d1", "message": {"id": "m1"}, "aiModerated": true});
  fake::publish(&mut ws, ROOM, "MessageDeletedEvent", deleted).await;
  assert_eq!(
    events.next().await,
    json!({"slug": "one", "kind": "deleted", "id": "m1", "ai": true})
  );

  let user = json!({"id": 7, "username": "someone", "slug": "someone"});
  let moderator =
    json!({"id": 0, "username": "moderator", "slug": "moderator"});
  let banned = json!({
    "id": "b1",
    "user": user,
    "banned_by": moderator,
    "permanent": false,
    "duration": 5,
    "expires_at": "2026-11-23T10:05:00+00:00",
  });
  fake::publish(&mut ws, ROOM, "UserBannedEvent", banned).await;
  assert_eq!(
    events.next().await,
    json!({
      "slug": "one",
      "kind": "banned",
      "user": {"id": 7, "username": "someone"},
      "by": "moderator",
      "permanent": false,
      "minutes": 5,
      "until": ::common::time::unix_ms("2026-11-23T10:05:00Z"),
    })
  );

  let unbanned = json!({"id": "u1", "user": user, "unbanned_by": moderator});
  fake::publish(&mut ws, ROOM, "UserUnbannedEvent", unbanned).await;
  assert_eq!(
    events.next().await,
    json!({
      "slug": "one",
      "kind": "unbanned",
      "user": {"id": 7, "username": "someone"},
      "by": "moderator",
    })
  );

  // What the page doesn't show is skipped.
  fake::publish(&mut ws, ROOM, "PinnedMessageDeletedEvent", json!([])).await;
  fake::publish(&mut ws, ROOM, "ChatroomClearEvent", json!({"id": "c1"})).await;
  assert_eq!(
    events.next().await,
    json!({"slug": "one", "kind": "cleared"})
  );

  // A dropped connection is retried, and shown.
  drop(ws);
  assert_eq!(events.status("one").await, "reconnecting");
  let _ws = centrifugo.accept(ROOM).await;
  assert_eq!(events.status("one").await, "live");

  // And all of it was journaled, raw, in one file: the endpoint's the same.
  let journal = server.dir.path().join("journal");
  assert!(!journal.join("two").exists());
  let files = live_files(&journal.join("one")).unwrap();
  assert_eq!(files.len(), 1);
  assert!(files[0].ends_with("kick-mod-5.journal"));
  let reader = Reader::open(&files[0], 0).unwrap();
  let header = reader.header().unwrap();
  assert_eq!(header.get("endpoint"), Some(centrifugo.url.as_str()));
  assert_eq!(header.get("channel"), Some("one"));
  let records: Vec<v0::Record> = reader
    .map(|entry| {
      let Record::Kick0(record) = entry.unwrap().record else {
        panic!("not Kick's")
      };
      assert_eq!(record.instance, "kick-mod-5");
      record
    })
    .collect();
  let open = Body::Open(centrifugo.url.clone());
  assert_eq!((records[0].connection, &records[0].body), (1, &open));
  let frames = |connection| {
    records
      .iter()
      .filter(move |r| r.connection == connection)
      .filter_map(|r| match &r.body {
        Body::Frame(frame) => Some(frame.as_str()),
        _ => None,
      })
  };
  assert!(frames(1).any(|frame| frame.contains(r#"\"id\":\"m1\""#)));
  let closed = records
    .iter()
    .position(|r| matches!(r.body, Body::Close(_)));
  let reopened = &records[closed.unwrap() + 1];
  assert_eq!((reopened.connection, &reopened.body), (2, &open));
  assert!(frames(2).next().is_some());
}

#[tokio::test]
async fn a_page_opened_later_sees_the_status_now() {
  let centrifugo = Centrifugo::start().await;
  let channels = r#"{"one": {"broadcaster_user_id": 1, "chatroom_id": 1000}}"#;
  let server = Server::start_with(channels, centrifugo.options()).await;
  let mut first = server.events().await;
  assert_eq!(first.status("one").await, "connecting");
  let _ws = centrifugo.accept(ROOM).await;
  assert_eq!(first.status("one").await, "live");
  let mut later = server.events().await;
  assert_eq!(later.status("one").await, "live");
}

const ONE: &str = r#"{"one": {"broadcaster_user_id": 1, "chatroom_id": null}}"#;

#[tokio::test]
async fn acts_as_the_moderator_and_logs_it() {
  let server = Server::start_with(ONE, Options::default()).await;
  server.sign_in().await;
  server
    .kick
    .answer("/moderation/bans", 200, r#"{"data":{},"message":"OK"}"#);
  server.kick.answer("/chat/m1", 204, "");
  let user = json!({"id": 9, "username": "someone"});

  let timeout = json!({
    "channel": "one",
    "action": "timeout",
    "minutes": 10,
    "user": user,
    "message_id": "m1",
    "reason": " spam ",
  });
  assert_eq!(server.act(timeout).await, (204, String::new()));
  let delete = json!({
    "channel": "one",
    "action": "delete",
    "user": user,
    "message_id": "m1",
  });
  assert_eq!(server.act(delete).await, (204, String::new()));
  let unban = json!({"channel": "one", "action": "unban", "user": user});
  assert_eq!(server.act(unban).await, (204, String::new()));

  // Who's signed in was asked once, at the sign-in.
  let requests = server.kick.requests();
  let sent: Vec<_> = requests
    .iter()
    .filter(|r| r.path != "/oauth/token" && r.path != "/users")
    .map(|r| {
      assert_eq!(r.bearer.as_deref(), Some("access-1"));
      let body = ::serde_json::from_str(&r.body).unwrap_or(Value::Null);
      (r.method.as_str(), r.path.as_str(), body)
    })
    .collect();
  let ban = json!({
    "broadcaster_user_id": 1,
    "user_id": 9,
    "duration": 10,
    "reason": "spam",
  });
  let unbanned = json!({"broadcaster_user_id": 1, "user_id": 9});
  assert_eq!(
    sent,
    [
      ("POST", "/moderation/bans", ban),
      ("DELETE", "/chat/m1", Value::Null),
      ("DELETE", "/moderation/bans", unbanned),
    ]
  );
  assert_eq!(requests.iter().filter(|r| r.path == "/users").count(), 1);

  let logged = server.modlog();
  let asked: Vec<_> = logged.iter().map(|e| &e.asked).collect();
  let target = Target {
    id: 9,
    username: "someone".into(),
  };
  assert_eq!(
    asked,
    [
      &Asked {
        channel: "one".into(),
        action: Action::Timeout { minutes: 10 },
        user: target.clone(),
        message_id: Some("m1".into()),
        reason: Some("spam".into()),
      },
      &Asked {
        channel: "one".into(),
        action: Action::Delete,
        user: target.clone(),
        message_id: Some("m1".into()),
        reason: None,
      },
      &Asked {
        channel: "one".into(),
        action: Action::Unban,
        user: target,
        message_id: None,
        reason: None,
      },
    ]
  );
  let moderator = Moderator {
    user_id: 7,
    name: Some("moderator".into()),
  };
  for entry in &logged {
    assert_eq!(entry.by, moderator);
    assert_eq!(entry.broadcaster_user_id, 1);
    assert_eq!(entry.answer, Answer::Done);
  }
}

#[tokio::test]
async fn says_and_logs_what_kick_refused() {
  let server = Server::start_with(ONE, Options::default()).await;
  server.sign_in().await;
  server
    .kick
    .answer("/moderation/bans", 403, r#"{"message":"Forbidden"}"#);
  let ban = json!({
    "channel": "one",
    "action": "ban",
    "user": {"id": 9, "username": "someone"},
  });
  assert_eq!(
    server.act(ban).await,
    (502, "Kick refused 403: Forbidden".into())
  );
  let logged = server.modlog();
  assert_eq!(logged.len(), 1);
  assert_eq!(
    logged[0].answer,
    Answer::Refused {
      status: Some(403),
      error: "Forbidden".into(),
    }
  );
}

#[tokio::test]
async fn refuses_what_it_cant_do() {
  let server = Server::start_with(ONE, Options::default()).await;
  let user = json!({"id": 9, "username": "someone"});
  let ban = json!({"channel": "one", "action": "ban", "user": user});
  assert_eq!(server.act(ban).await, (401, "sign in to moderate".into()));

  server.sign_in().await;
  let elsewhere = json!({"channel": "two", "action": "ban", "user": user});
  assert_eq!(
    server.act(elsewhere).await,
    (404, "no channel two in channels.json".into())
  );
  let long = json!({
    "channel": "one",
    "action": "timeout",
    "minutes": 10081,
    "user": user,
  });
  assert_eq!(server.act(long).await.0, 400);
  // None of them reached Kick, or the log.
  assert!(!server.kick.paths().contains(&"/moderation/bans".into()));
  assert_eq!(server.modlog(), []);

  let from_elsewhere = server
    .http
    .post(format!("{}/api/actions", server.base))
    .header(ORIGIN, "https://example.com")
    .json(&json!({"channel": "one", "action": "ban", "user": user}));
  let status = from_elsewhere.send().await.unwrap().status().as_u16();
  assert_eq!(status, 403);
}

#[tokio::test]
async fn looks_up_a_users_history() {
  let server = Server::start().await;
  let data = json!({
    "id": "m1",
    "content": "hello",
    "sender": {"id": 9, "username": "someone"},
  });
  let push = json!({
    "push": {
      "channel": ROOM,
      "pub": {
        "data": {
          "event": r"App\Events\ChatMessageEvent",
          "data": data.to_string(),
        },
      },
    },
  });
  let journal = format!(
    "journal kick.0.0 ; channel=one\u{1e}\n\
     1709251200001 kick-mod-1 1 frame {push}\u{1e}\n"
  );
  let file = "journal/one/live/2024-03-01/kick-mod-1.journal";
  server.dir.write(file, journal);
  let user = json!({"id": 9, "username": "someone"});
  let asked = json!({"channel": "one", "action": "ban", "user": user});
  let entry = json!({
    "at": 1709251200002_i64,
    "by": {"user_id": 7, "name": "moderator"},
    "broadcaster_user_id": 456,
    "message_id": null,
    "reason": null,
    "answer": {"result": "done"},
  });
  let mut entry = entry.as_object().unwrap().clone();
  entry.extend(asked.as_object().unwrap().clone());
  server
    .dir
    .write("modlog.jsonl", format!("{}\n", Value::from(entry)));

  let (status, body) = server.get("/api/history?channel=one&user=9", &[]).await;
  assert_eq!(status, 200, "{body}");
  let history: Value = ::serde_json::from_str(&body).unwrap();
  assert_eq!(history["username"], "someone");
  assert_eq!(history["messages"][0]["content"], "hello");
  assert_eq!(history["messages"][0]["received"], 1709251200001_i64);
  assert_eq!(history["actions"][0]["action"], "ban");
  assert_eq!(history["events"], json!([]));

  let (status, _) = server.get("/api/history?channel=one", &[]).await;
  assert_eq!(status, 400);
}
