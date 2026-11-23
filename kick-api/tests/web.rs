//! web.kick.com's chat history, against a local server standing in for
//! Kick's. The fixture's ids, names and messages are made up, in the shape
//! Kick answered (2026-11-24).

mod fake;

use {
  ::kick_api::web::{Error, Web, cursor_at},
  ::serde_json::json,
};

const HISTORY: &str = include_str!("fixtures/history.json");

fn web(server: &fake::Server) -> Web {
  Web::new(::reqwest::Client::new(), "kick-api-tests/0").with_base(&server.base)
}

#[tokio::test]
async fn a_page_of_history() {
  let server = fake::serve(|_| (200, HISTORY.into())).await;
  let page = web(&server).history(1000, None).await.unwrap();

  let request = &server.requests()[0];
  assert_eq!(request.path, "/chat/1000/history");
  assert!(request.query.is_empty());
  assert_eq!(request.headers["user-agent"], "kick-api-tests/0");
  assert_eq!(page.cursor.as_deref(), Some("1795462741000000"));
  assert_eq!(page.pinned_message, None);
  assert_eq!(page.rest, None);

  let [first, second] = &page.messages[..] else {
    panic!("two messages: {:?}", page.messages);
  };
  assert_eq!(first.id, "0b6e3c52-1f0d-4a8e-9c1e-2f4b8f0a1c01");
  assert_eq!(first.chat_id, Some(1000));
  assert_eq!(first.user_id, Some(42));
  assert_eq!(first.content, "hello [emote:1:wave]");
  assert_eq!(first.r#type.as_deref(), Some("message"));
  assert_eq!(
    first.metadata,
    Some(json!(r#"{"message_ref":"1795462741000"}"#))
  );
  assert_eq!(first.created_at, Some(1_795_462_742_000));
  assert_eq!(first.sender.id, 42);
  assert_eq!(first.sender.username, "Someone");
  let identity = first.sender.identity.as_ref().unwrap();
  assert_eq!(identity.color.as_deref(), Some("#00FF00"));
  assert_eq!(identity.badges[0].r#type, "subscriber");
  assert_eq!(first.rest, None);

  assert_eq!(second.r#type.as_deref(), Some("reply"));
  // What it doesn't name is kept.
  let rest = second.rest.as_ref().unwrap();
  assert_eq!(rest["something_new"], 1);
}

#[tokio::test]
async fn pages_back_by_cursor() {
  let empty = json!({
    "data": {"messages": [], "cursor": "", "pinned_message": null},
    "message": "OK",
  });
  let server = fake::serve(move |_| (200, empty.to_string())).await;
  let cursor = cursor_at(1_795_462_741_539);
  assert_eq!(cursor, "1795462741539000");
  let page = web(&server).history(1000, Some(&cursor)).await.unwrap();
  let query = &server.requests()[0].query;
  assert_eq!(query, &[("cursor".into(), cursor)]);
  // An empty cursor: the start of the history.
  assert!(page.messages.is_empty());
  assert_eq!(page.cursor, None);
}

#[tokio::test]
async fn says_why_not() {
  let blocked = r#"{"error": "Request blocked by security policy.",
    "reference": "0000"}"#;
  let server = fake::serve(move |_| (403, blocked.into())).await;
  let e = web(&server).history(1000, None).await.unwrap_err();
  assert_eq!(e.status(), Some(403));
  assert!(e.blocked());

  let server = fake::serve(|_| (404, "{}".into())).await;
  let e = web(&server).history(1000, None).await.unwrap_err();
  assert!(!e.blocked());

  let server = fake::serve(|_| (200, "<html>".into())).await;
  let e = web(&server).history(1000, None).await.unwrap_err();
  assert!(matches!(e, Error::Decode { raw, .. } if raw == "<html>"));
}
