//! What the page is told of a channel's connection. Events are in serve's
//! tests, end to end.

use {
  ::kick_api::realtime::{Item, Stage},
  ::kick_mod::chat::{State, What},
  ::std::time::Duration,
};

fn status(item: Item) -> Option<(State, Option<String>)> {
  match What::from_item(&item)? {
    What::Status { state, detail } => Some((state, detail)),
    other => panic!("not a status: {other:?}"),
  }
}

#[test]
fn live_once_subscribed() {
  let connected = Item::Connected {
    connection: 1,
    endpoint: "wss://realtime.example".into(),
  };
  assert_eq!(status(connected), None);
  let subscribed = Item::Subscribed {
    connection: 1,
    channel: "chatrooms.1000.v2".into(),
  };
  assert_eq!(status(subscribed), Some((State::Live, None)));
}

#[test]
fn says_why_its_down() {
  let failed = Item::ConnectFailed {
    stage: Stage::Token,
    error: "answered 503".into(),
    retry_in: Duration::from_secs(4),
  };
  let detail = "token: answered 503; again in 4.0s".to_owned();
  assert_eq!(status(failed), Some((State::Reconnecting, Some(detail))));
  let dropped = Item::Disconnected {
    connection: 1,
    code: None,
    reason: "idle timeout".into(),
  };
  let detail = Some("idle timeout".into());
  assert_eq!(status(dropped), Some((State::Reconnecting, detail)));
  let stopped = Item::Stopped {
    code: 3501,
    reason: "bad request".into(),
  };
  let detail = Some("3501 bad request".into());
  assert_eq!(status(stopped), Some((State::Stopped, detail)));
}

#[test]
fn unsubscribed_is_down_unless_resubscribing() {
  let unsubscribed = |resubscribing| Item::Unsubscribed {
    connection: 1,
    channel: "chatrooms.1000.v2".into(),
    code: 2500,
    reason: "insufficient state".into(),
    resubscribing,
  };
  let detail = Some("2500 insufficient state".into());
  assert_eq!(
    status(unsubscribed(true)),
    Some((State::Reconnecting, detail.clone()))
  );
  assert_eq!(status(unsubscribed(false)), Some((State::Stopped, detail)));
}
