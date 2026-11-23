//! Decoding pages of messages. The messages are made up, in the shape
//! linkers.ooo answered (2026-11-24).

use ::linkers_api::{Error, Id, Kind, RateLimit, parse};

const PAGE: &str = r#"{"messages":[
  {"id":4,"timestamp":"2026-11-08T20:05:00.000Z","user":"Someone",
   "message":"hello there","message_type":"MSG","channel":"Destinygg",
   "source":"linkers","user_flairs":["subscriber","flair1"],
   "subscription_tier":2,"subscription_tier_label":"Tier II"},
  {"id":3,"timestamp":"2026-11-08T20:04:59.000Z","user":"Mod",
   "message":"someone banned by Mod","message_type":"BAN",
   "channel":"Destinygg","user_flairs":["protected"],
   "subscription_tier":null,"subscription_tier_label":null},
  {"id":"a274480534","timestamp":"2025-12-20T12:00:56.123Z","user":"old",
   "message":"hi","message_type":"MSG","source":"rustlesearch",
   "somethingNew":1}],
  "total":3,"totalCapped":false,"limit":500,"offset":0,
  "sources":["linkers","rustlesearch"]}"#;

#[test]
fn decodes_a_page() {
  let page = parse(PAGE).unwrap();
  assert_eq!(
    (page.total, page.total_capped, page.limit, page.offset),
    (Some(3), Some(false), Some(500), Some(0))
  );
  assert_eq!(page.rest.as_ref().unwrap()["sources"][1], "rustlesearch");

  let [said, banned, relayed] = &page.messages[..] else {
    panic!("three messages: {:?}", page.messages);
  };
  assert_eq!(said.id, Id::Number(4));
  assert_eq!(said.timestamp, 1_794_168_300_000);
  assert_eq!(
    (said.user.as_str(), said.message.as_str()),
    ("Someone", "hello there")
  );
  assert_eq!(said.message_type, Kind::Msg);
  assert_eq!(said.channel.as_deref(), Some("Destinygg"));
  assert_eq!(said.source.as_deref(), Some("linkers"));
  assert_eq!(
    said.user_flairs.as_deref(),
    Some(&["subscriber".into(), "flair1".into()][..])
  );
  assert_eq!(said.subscription_tier, Some(2));
  assert_eq!(said.subscription_tier_label.as_deref(), Some("Tier II"));
  assert_eq!(said.rest, None);

  assert_eq!(banned.message_type, Kind::Ban);
  assert_eq!(
    (banned.source.as_deref(), banned.subscription_tier),
    (None, None)
  );

  assert_eq!(relayed.id, Id::Text("a274480534".into()));
  assert_eq!(relayed.timestamp % 1000, 123);
  // What it doesn't name is kept.
  assert_eq!(relayed.rest.as_ref().unwrap()["somethingNew"], 1);
}

#[test]
fn knows_every_kind_and_keeps_new_ones() {
  for name in [
    "MSG",
    "BAN",
    "MUTE",
    "SUBSCRIPTION",
    "GIFTSUB",
    "MASSGIFT",
    "DONATION",
  ] {
    let kind = Kind::named(name);
    assert!(!matches!(kind, Kind::Other(_)), "{name}");
    assert_eq!(kind.as_str(), name);
  }
  assert_eq!(Kind::named("POLL"), Kind::Other("POLL".into()));
  assert_eq!(Kind::named("POLL").as_str(), "POLL");
}

#[test]
fn says_what_didnt_decode() {
  for body in [
    r#"{"error":"Failed"}"#,
    "<html>",
    r#"{"messages":[{"id":1,"timestamp":"yesterday","user":"a",
      "message":"b","message_type":"MSG"}]}"#,
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
  let headers = [
    ("X-RateLimit-Limit", "60"),
    ("X-RateLimit-Remaining", "50"),
    ("X-RateLimit-Reset", "1795471496"),
    ("Content-Type", "application/json"),
  ];
  assert_eq!(
    RateLimit::from_headers(headers),
    Some(RateLimit {
      limit: Some(60),
      remaining: Some(50),
      reset: Some(1_795_471_496),
    })
  );
  let partial = RateLimit::from_headers([("x-ratelimit-remaining", "lots")]);
  assert_eq!(partial, Some(RateLimit::default()));
  assert_eq!(
    RateLimit::from_headers([("Content-Type", "text/html")]),
    None
  );
}
