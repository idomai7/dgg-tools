//! Fetching, against a local server standing in for linkers.ooo.

mod fake;

use ::linkers_api::{Api, Error, Kind, Query};

const PAGE: &str = r#"{"messages":[
  {"id":4,"timestamp":"2026-11-08T20:05:00.000Z","user":"Someone",
   "message":"hello","message_type":"MSG"}],
  "total":1,"totalCapped":false,"limit":500,"offset":0}"#;

fn api(server: &fake::Server) -> Api {
  Api::new(::reqwest::Client::new(), "linkers-api-tests/0")
    .with_base(&server.base)
}

fn query() -> Query {
  Query {
    message_type: Kind::Msg,
    sources: Some("all".into()),
    end_time: Some(1_794_168_300_999),
    limit: 500,
  }
}

#[tokio::test]
async fn asks_for_a_page() {
  let server = fake::serve(|_| {
    let headers = vec![
      ("X-RateLimit-Limit", "60".into()),
      ("X-RateLimit-Remaining", "59".into()),
    ];
    (200, headers, PAGE.into())
  })
  .await;
  let answer = api(&server).messages(&query()).await.unwrap();

  let request = &server.requests()[0];
  assert_eq!(request.path, "/api/messages");
  assert_eq!(
    request.query,
    [
      ("messageType".into(), "MSG".into()),
      ("limit".into(), "500".into()),
      ("sources".into(), "all".into()),
      ("endTime".into(), "2026-11-08T20:05:00.999Z".into()),
    ]
  );
  assert_eq!(request.headers["user-agent"], "linkers-api-tests/0");
  assert!(!request.headers.contains_key("cookie"));
  assert_eq!(answer.messages.messages[0].message, "hello");
  let limit = answer.rate_limit.unwrap();
  assert_eq!((limit.limit, limit.remaining), (Some(60), Some(59)));
}

#[tokio::test]
async fn as_a_logged_in_user() {
  let server = fake::serve(|_| (200, vec![], PAGE.into())).await;
  let api = api(&server).with_session(Some("s3cret".into()));
  let query = Query {
    message_type: Kind::Ban,
    sources: None,
    end_time: None,
    limit: 10,
  };
  let answer = api.messages(&query).await.unwrap();
  let request = &server.requests()[0];
  assert_eq!(request.headers["cookie"], "lk_session=s3cret");
  assert_eq!(
    request.query,
    [
      ("messageType".into(), "BAN".into()),
      ("limit".into(), "10".into()),
    ]
  );
  assert_eq!(answer.rate_limit, None);
}

#[tokio::test]
async fn says_why_not() {
  let server = fake::serve(|_| (429, vec![], "slow down".into())).await;
  let e = api(&server).messages(&query()).await.unwrap_err();
  assert_eq!(e.status(), Some(429));
  assert_eq!(e.to_string(), "HTTP 429: slow down");

  let server = fake::serve(|_| (200, vec![], "<html>".into())).await;
  let e = api(&server).messages(&query()).await.unwrap_err();
  assert!(matches!(e, Error::Decode { raw, .. } if raw == "<html>"));
}
