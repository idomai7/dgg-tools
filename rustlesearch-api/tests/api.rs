//! Fetching, against a local server standing in for rustlesearch.dev.

mod fake;

use ::rustlesearch_api::{Api, Error, RateLimit};

const SEARCH: &str = r#"{"type":"success","error":null,"data":{"messages":[
  {"channel":"Destinygg","text":"hello","ts":"2026-11-08T16:54:48.711Z",
   "username":"someone","searchAfter":1794156888711}]}}"#;

fn api(server: &fake::Server) -> Api {
  Api::new(::reqwest::Client::new(), "rustlesearch-api-tests/0")
    .with_base(&server.base)
}

#[tokio::test]
async fn searches_before_a_time() {
  let server = fake::serve(|_| {
    let limit = ("Ratelimit", "limit=200, remaining=191, reset=21864".into());
    (200, vec![limit], SEARCH.into())
  })
  .await;
  let answer = api(&server).search("Destinygg", 1_794_156_890_000).await;
  let answer = answer.unwrap();

  let request = &server.requests()[0];
  assert_eq!(request.path, "/anon/search");
  assert_eq!(
    request.query,
    [
      ("channel".into(), "Destinygg".into()),
      ("search_after".into(), "1794156890000".into()),
    ]
  );
  assert_eq!(request.headers["user-agent"], "rustlesearch-api-tests/0");
  assert_eq!(answer.search.messages[0].text, "hello");
  let limit = answer.rate_limit.unwrap();
  assert_eq!(limit.remaining, Some(191));
}

#[tokio::test]
async fn without_a_rate_limit_header() {
  let server = fake::serve(|_| (200, vec![], SEARCH.into())).await;
  let answer = api(&server).search("Destinygg", 0).await.unwrap();
  assert_eq!(answer.rate_limit, None::<RateLimit>);
}

#[tokio::test]
async fn says_why_not() {
  let server = fake::serve(|_| (429, vec![], "slow down".into())).await;
  let e = api(&server).search("Destinygg", 0).await.unwrap_err();
  assert_eq!(e.status(), Some(429));
  assert_eq!(e.to_string(), "HTTP 429: slow down");

  let server = fake::serve(|_| (200, vec![], "<html>".into())).await;
  let e = api(&server).search("Destinygg", 0).await.unwrap_err();
  assert!(matches!(e, Error::Decode { raw, .. } if raw == "<html>"));
}
