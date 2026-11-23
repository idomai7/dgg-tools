//! The mod log. Ids and names are made up.

use {
  ::common::temp_dir::TempDir,
  ::kick_mod::{
    actions::{Action, Asked, Target},
    modlog::{self, Answer, Entry, Moderator},
  },
  ::serde_json::{Value, json},
  ::std::fs,
};

fn entry(action: Action, answer: Answer) -> Entry {
  Entry {
    at: 1_700_000_000_000,
    by: Moderator {
      user_id: 7,
      name: Some("moderator".into()),
    },
    broadcaster_user_id: 1,
    asked: Asked {
      channel: "one".into(),
      action,
      user: Target {
        id: 9,
        username: "someone".into(),
      },
      message_id: Some("m1".into()),
      reason: Some("spam".into()),
    },
    answer,
  }
}

#[test]
fn appends_a_line_each() {
  let dir = TempDir::new("kick-mod-modlog");
  let path = dir.join("kick-mod").join("modlog.jsonl");
  let refused = Answer::Refused {
    status: Some(403),
    error: "Forbidden".into(),
  };
  let entries = [
    entry(Action::Timeout { minutes: 10 }, Answer::Done),
    entry(Action::Ban, refused),
  ];
  for entry in &entries {
    modlog::append(&path, entry).unwrap();
  }
  let text = fs::read_to_string(&path).unwrap();
  assert_eq!(modlog::parse(&text).unwrap(), entries);
  let lines: Vec<Value> = text
    .lines()
    .map(|line| ::serde_json::from_str(line).unwrap())
    .collect();
  assert_eq!(
    lines,
    [
      json!({
        "at": 1_700_000_000_000_i64,
        "by": {"user_id": 7, "name": "moderator"},
        "broadcaster_user_id": 1,
        "channel": "one",
        "action": "timeout",
        "minutes": 10,
        "user": {"id": 9, "username": "someone"},
        "message_id": "m1",
        "reason": "spam",
        "answer": {"result": "done"},
      }),
      json!({
        "at": 1_700_000_000_000_i64,
        "by": {"user_id": 7, "name": "moderator"},
        "broadcaster_user_id": 1,
        "channel": "one",
        "action": "ban",
        "user": {"id": 9, "username": "someone"},
        "message_id": "m1",
        "reason": "spam",
        "answer": {"result": "refused", "status": 403, "error": "Forbidden"},
      }),
    ]
  );
}

#[test]
fn says_which_line_is_broken() {
  let line = r#"{"at": 1}"#;
  let error = modlog::parse(&format!("\n{line}\n")).unwrap_err();
  assert!(error.starts_with("line 2: "), "{error}");
}
