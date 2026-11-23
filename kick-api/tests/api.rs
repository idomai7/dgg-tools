//! The public API and its token, against a local server standing in for
//! Kick's. Ids, names and tokens are made up.

mod fake;

use {
  ::common::time::now_ms,
  ::kick_api::official::{
    Api, Error,
    api::{Ban, Chat, Sender},
    oauth::{App, Token},
  },
  ::serde_json::json,
  ::std::sync::{Arc, Mutex},
};

fn token(access: &str, refresh: Option<&str>, expires_at: i64) -> Token {
  Token {
    access_token: access.into(),
    refresh_token: refresh.map(Into::into),
    token_type: Some("Bearer".into()),
    scope: None,
    expires_at,
  }
}

/// A fresh token, for an hour.
fn fresh() -> Token {
  token("access-0", Some("refresh-0"), now_ms() + 3_600_000)
}

/// Kick's answer to a refresh: `access-1`, for an hour.
const REFRESHED: &str = r#"{"access_token":"access-1","token_type":"Bearer",
  "refresh_token":"refresh-1","expires_in":3600}"#;

/// As a user with `token`, served by `server`, and the tokens it kept.
fn user(server: &fake::Server, token: Token) -> (Api, Arc<Mutex<Vec<Token>>>) {
  let mut app = App::new("app-id", "app-secret");
  app.base = server.base.clone();
  let kept = Arc::new(Mutex::new(Vec::new()));
  let keep = kept.clone();
  let api = Api::user(::reqwest::Client::new(), app, token, move |token| {
    keep.lock().unwrap().push(token.clone())
  });
  (api.with_base(&server.base), kept)
}

/// Answers every request with `status` and `data` in Kick's envelope.
async fn answer(status: u16, data: ::serde_json::Value) -> fake::Server {
  let body = json!({ "data": data, "message": "OK" }).to_string();
  fake::serve(move |_| (status, body.clone())).await
}

/// Answers a refresh with [`REFRESHED`], and anything else with `answer`.
async fn refreshing(
  answer: impl Fn(&fake::Request) -> (u16, String) + Send + Sync + 'static,
) -> fake::Server {
  fake::serve(move |request| match &*request.path {
    "/oauth/token" => (200, REFRESHED.into()),
    _ => answer(request),
  })
  .await
}

fn query(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
  pairs.iter().map(|&(k, v)| (k.into(), v.into())).collect()
}

#[tokio::test]
async fn users() {
  let server = answer(
    200,
    json!([{ "user_id": 1, "name": "user1", "profile_picture": "p",
             "email": "", "new": true }]),
  )
  .await;
  let (api, _) = user(&server, fresh());
  let users = api.users(&[1, 2]).await.unwrap();
  assert_eq!(users[0].user_id, 1);
  assert_eq!(users[0].name.as_deref(), Some("user1"));
  assert_eq!(users[0].rest.as_ref().unwrap()["new"], true);
  let request = &server.requests()[0];
  assert_eq!((&*request.method, &*request.path), ("GET", "/users"));
  assert_eq!(request.query, query(&[("id", "1"), ("id", "2")]));
  assert_eq!(request.bearer(), Some("access-0"));

  api.users(&[]).await.unwrap();
  assert!(server.requests()[1].query.is_empty());
}

#[tokio::test]
async fn channels() {
  let server = answer(
    200,
    json!([{
      "broadcaster_user_id": 3000, "slug": "channel",
      "stream_title": "title",
      "category": { "id": 1, "name": "Just Chatting", "thumbnail": "t" },
      "stream": { "is_live": true, "viewer_count": 5,
                  "start_time": "2026-11-19T12:00:00Z", "custom_tags": [] },
    }]),
  )
  .await;
  let (api, _) = user(&server, fresh());
  let channels = api.channels_by_slug(&["channel", "other"]).await.unwrap();
  let channel = &channels[0];
  assert_eq!(channel.broadcaster_user_id, 3000);
  assert_eq!(channel.category.as_ref().unwrap().id, Some(1));
  let stream = channel.stream.as_ref().unwrap();
  assert_eq!(stream.start_time, Some(1_795_089_600_000));
  assert_eq!((stream.is_live, stream.rest.as_ref()), (Some(true), None));
  assert_eq!(channel.rest, None);
  assert_eq!(
    server.requests()[0].query,
    query(&[("slug", "channel"), ("slug", "other")])
  );

  api.channels_by_id(&[3000]).await.unwrap();
  let request = &server.requests()[1];
  assert_eq!(request.path, "/channels");
  assert_eq!(request.query, query(&[("broadcaster_user_id", "3000")]));
}

#[tokio::test]
async fn sends_chat() {
  let server =
    answer(200, json!({ "is_sent": true, "message_id": "m-1" })).await;
  let (api, _) = user(&server, fresh());
  let chat = Chat {
    content: "hi".into(),
    r#type: Sender::User,
    broadcaster_user_id: Some(3000),
    reply_to_message_id: None,
  };
  let sent = api.send_chat(&chat).await.unwrap();
  assert_eq!(
    (sent.is_sent, sent.message_id.as_deref()),
    (true, Some("m-1"))
  );
  let request = &server.requests()[0];
  assert_eq!((&*request.method, &*request.path), ("POST", "/chat"));
  assert_eq!(
    request.json(),
    json!({ "content": "hi", "type": "user", "broadcaster_user_id": 3000 })
  );

  let bot = Chat {
    r#type: Sender::Bot,
    broadcaster_user_id: None,
    reply_to_message_id: Some("m-0".into()),
    ..chat
  };
  api.send_chat(&bot).await.unwrap();
  assert_eq!(
    server.requests()[1].json(),
    json!({ "content": "hi", "type": "bot", "reply_to_message_id": "m-0" })
  );
}

#[tokio::test]
async fn moderates() {
  let server = fake::serve(|request| match &*request.method {
    "DELETE" if request.path.starts_with("/chat/") => (204, String::new()),
    _ => (200, r#"{"data":{},"message":"OK"}"#.into()),
  })
  .await;
  let (api, _) = user(&server, fresh());
  api.delete_message("m-1").await.unwrap();
  let timeout = Ban {
    broadcaster_user_id: 3000,
    user_id: 1001,
    duration: Some(10),
    reason: Some("spam".into()),
  };
  api.ban(&timeout).await.unwrap();
  let ban = Ban {
    duration: None,
    reason: None,
    ..timeout
  };
  api.ban(&ban).await.unwrap();
  api.unban(3000, 1001).await.unwrap();

  let requests = server.requests();
  let sent: Vec<_> = requests
    .iter()
    .map(|r| (&*r.method, &*r.path, r.bearer()))
    .collect();
  assert_eq!(
    sent,
    [
      ("DELETE", "/chat/m-1", Some("access-0")),
      ("POST", "/moderation/bans", Some("access-0")),
      ("POST", "/moderation/bans", Some("access-0")),
      ("DELETE", "/moderation/bans", Some("access-0")),
    ]
  );
  assert!(requests[0].body.is_empty());
  assert_eq!(
    requests[1].json(),
    json!({ "broadcaster_user_id": 3000, "user_id": 1001,
            "duration": 10, "reason": "spam" })
  );
  assert_eq!(
    requests[2].json(),
    json!({ "broadcaster_user_id": 3000, "user_id": 1001 })
  );
  assert_eq!(
    requests[3].json(),
    json!({ "broadcaster_user_id": 3000, "user_id": 1001 })
  );
}

#[tokio::test]
async fn refreshes_before_it_expires() {
  let server =
    refreshing(|_| (200, r#"{"data":[],"message":"OK"}"#.into())).await;
  let expiring = token("access-0", Some("refresh-0"), now_ms() + 30_000);
  let (api, kept) = user(&server, expiring);
  api.users(&[]).await.unwrap();
  assert_eq!(server.paths(), ["/oauth/token", "/users"]);
  let requests = server.requests();
  assert_eq!(requests[0].form()["refresh_token"], "refresh-0");
  assert_eq!(requests[1].bearer(), Some("access-1"));
  let token = api.token().await.unwrap();
  assert_eq!(token.access_token, "access-1");
  assert_eq!(*kept.lock().unwrap(), [token]);

  // Now fresh: no refresh.
  api.users(&[]).await.unwrap();
  assert_eq!(server.paths().len(), 3);
}

#[tokio::test]
async fn retries_once_after_a_401() {
  let server = refreshing(|request| match request.bearer() {
    Some("access-0") => (401, r#"{"data":{},"message":"Unauthorized"}"#.into()),
    _ => (200, r#"{"data":[],"message":"OK"}"#.into()),
  })
  .await;
  let (api, kept) = user(&server, fresh());
  api.users(&[]).await.unwrap();
  assert_eq!(server.paths(), ["/users", "/oauth/token", "/users"]);
  assert_eq!(kept.lock().unwrap().len(), 1);

  // Refused again after the refresh: the 401 is the answer.
  let server = refreshing(|_| (401, "{}".into())).await;
  let (api, kept) = user(&server, fresh());
  let error = api.users(&[]).await.unwrap_err();
  assert_eq!(error.status(), Some(401));
  assert_eq!(server.paths(), ["/users", "/oauth/token", "/users"]);
  assert_eq!(kept.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn refreshes_when_asked() {
  let server = refreshing(|_| panic!("only refreshes")).await;
  let (api, kept) = user(&server, fresh());
  let token = api.refresh().await.unwrap();
  assert_eq!(token.refresh_token.as_deref(), Some("refresh-1"));
  assert!(token.expires_at > now_ms() + 3_500_000);
  assert_eq!(api.token().await, Some(token.clone()));
  assert_eq!(*kept.lock().unwrap(), [token]);

  // An answer without a refresh token keeps the last one.
  let server = fake::serve(|_| {
    (200, r#"{"access_token":"access-2","expires_in":60}"#.into())
  })
  .await;
  let (api, _) = user(&server, fresh());
  let token = api.refresh().await.unwrap();
  assert_eq!(token.access_token, "access-2");
  assert_eq!(token.refresh_token.as_deref(), Some("refresh-0"));
}

#[tokio::test]
async fn a_failed_refresh_changes_nothing() {
  let server =
    fake::serve(|_| (400, r#"{"error":"invalid_grant"}"#.into())).await;
  let token = fresh();
  let (api, kept) = user(&server, token.clone());
  assert_eq!(api.refresh().await.unwrap_err().status(), Some(400));
  assert_eq!(api.token().await, Some(token));
  assert!(kept.lock().unwrap().is_empty());
  assert_eq!(server.paths(), ["/oauth/token"]);
}

#[tokio::test]
async fn calls_at_once_share_a_refresh() {
  let server =
    refreshing(|_| (200, r#"{"data":[],"message":"OK"}"#.into())).await;
  let expired = token("access-0", Some("refresh-0"), 0);
  let (api, kept) = user(&server, expired);
  let (a, b, c) = ::tokio::join!(api.users(&[]), api.users(&[]), api.refresh());
  a.unwrap();
  b.unwrap();
  c.unwrap();
  let refreshes = server
    .paths()
    .iter()
    .filter(|p| *p == "/oauth/token")
    .count();
  // The explicit refresh may run before or after the calls' one.
  assert!((1..=2).contains(&refreshes), "{:?}", server.paths());
  assert_eq!(kept.lock().unwrap().len(), refreshes);

  let server =
    refreshing(|_| (200, r#"{"data":[],"message":"OK"}"#.into())).await;
  let (api, _) = user(&server, token("access-0", Some("refresh-0"), 0));
  let (a, b) = ::tokio::join!(api.users(&[]), api.users(&[]));
  a.unwrap();
  b.unwrap();
  assert_eq!(server.paths(), ["/oauth/token", "/users", "/users"]);
}

#[tokio::test]
async fn no_refresh_token_means_signing_in() {
  let server = refreshing(|_| panic!("nothing to send")).await;
  let (api, _) = user(&server, token("access-0", None, 0));
  assert!(matches!(api.users(&[]).await, Err(Error::NoRefreshToken)));
  assert!(server.paths().is_empty());
}

#[tokio::test]
async fn as_the_app() {
  let server = fake::serve(|request| match &*request.path {
    "/oauth/token" => {
      (200, r#"{"access_token":"app-1","expires_in":3600}"#.into())
    }
    _ => (200, r#"{"data":[],"message":"OK"}"#.into()),
  })
  .await;
  let mut app = App::new("app-id", "app-secret");
  app.base = server.base.clone();
  let api = Api::app(::reqwest::Client::new(), app).with_base(&server.base);
  assert_eq!(api.token().await, None);
  api.channels_by_slug(&["channel"]).await.unwrap();
  api.channels_by_slug(&["channel"]).await.unwrap();
  assert_eq!(server.paths(), ["/oauth/token", "/channels", "/channels"]);
  let requests = server.requests();
  assert_eq!(requests[0].form()["grant_type"], "client_credentials");
  assert_eq!(requests[2].bearer(), Some("app-1"));
}

#[tokio::test]
async fn errors() {
  let server = fake::serve(|request| match &*request.path {
    "/users" => (429, r#"{"data":{},"message":"Too Many Requests"}"#.into()),
    "/chat" => (403, "<html>Attention Required! | Cloudflare</html>".into()),
    _ => (403, r#"{"data":{},"message":"Forbidden"}"#.into()),
  })
  .await;
  let (api, _) = user(&server, fresh());
  let limited = api.users(&[]).await.unwrap_err();
  assert!(limited.rate_limited() && !limited.cloudflare());
  assert_eq!(limited.message().as_deref(), Some("Too Many Requests"));
  let chat = Chat {
    content: "hi".into(),
    r#type: Sender::Bot,
    broadcaster_user_id: None,
    reply_to_message_id: None,
  };
  let blocked = api.send_chat(&chat).await.unwrap_err();
  assert!(blocked.cloudflare() && blocked.message().is_none());
  let forbidden = api.unban(1, 2).await.unwrap_err();
  assert!(!forbidden.cloudflare());
  assert_eq!(forbidden.message().as_deref(), Some("Forbidden"));

  let server = fake::serve(|_| (200, r#"{"message":"OK"}"#.into())).await;
  let (api, _) = user(&server, fresh());
  match api.users(&[]).await {
    Err(Error::Decode { raw, .. }) => assert_eq!(raw, r#"{"message":"OK"}"#),
    other => panic!("{other:?}"),
  }
}
