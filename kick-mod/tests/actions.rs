//! Checking actions before Kick is asked. Ids and names are made up.

use ::kick_mod::actions::{Action, Asked, MAX_REASON, MAX_TIMEOUT, Target};

fn asked(action: Action) -> Asked {
  Asked {
    channel: "one".into(),
    action,
    user: Target {
      id: 9,
      username: "someone".into(),
    },
    message_id: None,
    reason: None,
  }
}

#[test]
fn a_blank_reason_is_none() {
  let mut ban = asked(Action::Ban);
  ban.reason = Some("  ".into());
  assert_eq!(ban.check().unwrap().reason, None);
  let mut ban = asked(Action::Ban);
  ban.reason = Some(" spam\n".into());
  assert_eq!(ban.check().unwrap().reason.as_deref(), Some("spam"));
}

#[test]
fn reasons_are_at_most_100_characters() {
  let mut ban = asked(Action::Ban);
  ban.reason = Some("é".repeat(MAX_REASON));
  assert!(ban.clone().check().is_ok());
  ban.reason = Some("é".repeat(MAX_REASON + 1));
  assert!(ban.check().is_err());
}

#[test]
fn timeouts_are_a_minute_to_a_week() {
  for minutes in [1, MAX_TIMEOUT] {
    assert!(asked(Action::Timeout { minutes }).check().is_ok());
  }
  for minutes in [0, MAX_TIMEOUT + 1] {
    assert!(asked(Action::Timeout { minutes }).check().is_err());
  }
}

#[test]
fn a_delete_needs_its_message() {
  assert!(asked(Action::Delete).check().is_err());
  let mut delete = asked(Action::Delete);
  delete.message_id = Some("m1".into());
  assert!(delete.check().is_ok());
}

#[test]
fn reads_as_the_page_sends_it() {
  let sent = r#"{"channel": "one", "action": "timeout", "minutes": 10,
    "user": {"id": 9, "username": "someone"}, "message_id": "m1"}"#;
  let mut expected = asked(Action::Timeout { minutes: 10 });
  expected.message_id = Some("m1".into());
  assert_eq!(::serde_json::from_str::<Asked>(sent).unwrap(), expected);
}
