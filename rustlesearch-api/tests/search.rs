//! Decoding searches. The messages are made up, in the shape
//! rustlesearch.dev answered (2026-11-08).

use ::rustlesearch_api::{Error, RateLimit, parse};

const SEARCH: &str = r#"{"type":"success","error":null,"data":{"count":2,
  "countLimit":10000,"messages":[
  {"channel":"Destinygg","text":"hello","ts":"2026-11-08T16:54:48.711Z",
   "username":"someone","searchAfter":1794156888711},
  {"channel":"Destinygg","text":"hi  there","ts":"2016-01-01T23:58:03.000Z",
   "username":"old","searchAfter":1451692683000,"somethingNew":1}]}}"#;

#[test]
fn decodes_a_search() {
  let search = parse(SEARCH).unwrap();
  let rest = search.rest.as_ref().unwrap();
  assert_eq!(
    (&rest["count"], &rest["countLimit"]),
    (&2.into(), &10000.into())
  );

  let [first, second] = &search.messages[..] else {
    panic!("two messages: {:?}", search.messages);
  };
  assert_eq!(first.channel.as_deref(), Some("Destinygg"));
  assert_eq!(first.username, "someone");
  assert_eq!(first.text, "hello");
  assert_eq!(first.ts, 1_794_156_888_711);
  assert_eq!(first.search_after, Some(1_794_156_888_711));
  assert_eq!(first.rest, None);

  assert_eq!(
    (second.ts, second.text.as_str()),
    (1_451_692_683_000, "hi  there")
  );
  // What it doesn't name is kept.
  assert_eq!(second.rest.as_ref().unwrap()["somethingNew"], 1);
}

#[test]
fn past_the_start_there_are_none() {
  let empty = r#"{"type":"success","data":{"messages":[]}}"#;
  assert!(parse(empty).unwrap().messages.is_empty());
}

#[test]
fn says_what_didnt_decode() {
  for body in [
    r#"{"type":"error"}"#,
    "<html>",
    r#"{"data":{"messages":[{"username":"a","text":"b","ts":"yesterday"}]}}"#,
  ] {
    let e = parse(body).unwrap_err();
    assert!(
      matches!(&e, Error::Decode { raw, .. } if raw == body),
      "{e}"
    );
  }
}

#[test]
fn reads_rate_limits() {
  let header = "limit=200, remaining=191, reset=21864";
  assert_eq!(
    RateLimit::parse(header),
    RateLimit {
      limit: Some(200),
      remaining: Some(191),
      reset: Some(21864),
    }
  );
  let partial = RateLimit::parse("limit=200, remaining=lots");
  assert_eq!((partial.limit, partial.remaining), (Some(200), None));
}
