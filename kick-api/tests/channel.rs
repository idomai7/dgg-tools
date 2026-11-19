use ::kick_api::channel::{self, Channel, Chatroom};

/// What the lookup answered for a channel, cut down and anonymised.
const CHANNEL: &str = include_str!("fixtures/channel.json");

#[test]
fn url() {
  assert_eq!(
    channel::url("channel"),
    "https://kick.com/api/v2/channels/channel"
  );
}

#[test]
fn ids() {
  assert_eq!(
    channel::parse(CHANNEL).unwrap(),
    Channel {
      id: 2000,
      user_id: 3000,
      slug: "channel".into(),
      chatroom: Chatroom { id: 1000 },
    }
  );
}

#[test]
fn blocked() {
  // What Cloudflare answers a client it refuses.
  let blocked = r#"{"error":"Request blocked by security policy."}"#;
  assert!(channel::parse(blocked).is_err());
}
