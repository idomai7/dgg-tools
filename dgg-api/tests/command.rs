use {
  ::dgg_api::chat::protocol::command::{BanLength, Command, Poll, SubOnly},
  ::serde_json::{Value, json},
  ::std::time::Duration,
};

/// `command`'s frame, split into its name and parsed payload.
fn sent(command: Command) -> (String, Value) {
  let frame = command.encode();
  let (name, payload) = frame.split_once(' ').unwrap();
  (name.into(), ::serde_json::from_str(payload).unwrap())
}

#[test]
fn messages() {
  assert_eq!(
    sent(Command::Msg("hello \"there\"".into())),
    ("MSG".into(), json!({ "data": "hello \"there\"" }))
  );
  assert_eq!(
    sent(Command::PrivMsg {
      nick: "user1".into(),
      data: "hi".into(),
    }),
    ("PRIVMSG".into(), json!({ "nick": "user1", "data": "hi" }))
  );
  assert_eq!(
    sent(Command::CastVote("2".into())),
    ("CASTVOTE".into(), json!({ "vote": "2" }))
  );
}

#[test]
fn durations_go_as_nanoseconds() {
  assert_eq!(
    sent(Command::Mute {
      nick: "user1".into(),
      duration: Some(Duration::from_secs(600)),
    }),
    (
      "MUTE".into(),
      json!({ "data": "user1", "duration": 600_000_000_000_i64 })
    )
  );
  // Without one (or with zero), the server's default.
  for duration in [None, Some(Duration::ZERO)] {
    let (_, payload) = sent(Command::Mute {
      nick: "user1".into(),
      duration,
    });
    assert_eq!(payload, json!({ "data": "user1" }));
  }
  assert_eq!(
    sent(Command::Ban {
      nick: "user1".into(),
      reason: "spam".into(),
      length: BanLength::For(Duration::from_secs(3600)),
      ip: false,
    }),
    (
      "BAN".into(),
      json!({
        "nick": "user1",
        "reason": "spam",
        "banip": false,
        "duration": 3_600_000_000_000_i64,
      })
    )
  );
  let (_, permanent) = sent(Command::Ban {
    nick: "user1".into(),
    reason: "spam".into(),
    length: BanLength::Permanent,
    ip: true,
  });
  assert_eq!(
    permanent,
    json!({
      "nick": "user1",
      "reason": "spam",
      "banip": true,
      "ispermanent": true,
    })
  );
}

#[test]
fn moderation() {
  assert_eq!(
    sent(Command::Unmute("user1".into())),
    ("UNMUTE".into(), json!({ "data": "user1" }))
  );
  assert_eq!(
    sent(Command::Unban("user1".into())),
    ("UNBAN".into(), json!({ "data": "user1" }))
  );
  assert_eq!(
    sent(Command::SubOnly(SubOnly::Tier(2))),
    ("SUBONLY".into(), json!({ "data": "on", "tier": 2 }))
  );
  assert_eq!(
    sent(Command::SubOnly(SubOnly::Off)),
    ("SUBONLY".into(), json!({ "data": "off" }))
  );
  assert_eq!(sent(Command::Unpin), ("PIN".into(), json!({ "data": "" })));
  assert_eq!(
    sent(Command::AddPhrase("bad.*phrase".into())),
    ("ADDPHRASE".into(), json!({ "data": "bad.*phrase" }))
  );
  assert_eq!(sent(Command::Die), ("DIE".into(), json!({ "data": "" })));
  assert_eq!(sent(Command::StopPoll), ("STOPPOLL".into(), json!({})));
}

#[test]
fn polls_and_spotlights() {
  assert_eq!(
    sent(Command::StartPoll(Poll {
      question: "Which?".into(),
      options: vec!["this".into(), "that".into()],
      time: Duration::from_secs(30),
      weighted: true,
      hidden: false,
    })),
    (
      "STARTPOLL".into(),
      json!({
        "question": "Which?",
        "options": ["this", "that"],
        "time": 30_000,
        "weighted": true,
        "hidden": false,
      })
    )
  );
  assert_eq!(
    sent(Command::Spotlight {
      nick: "user1".into(),
      message_timestamp: 1794203923846,
      data: "message 1".into(),
    }),
    (
      "SPOTLIGHT".into(),
      json!({
        "nick": "user1",
        "messageTimestamp": 1794203923846_i64,
        "data": "message 1",
      })
    )
  );
}
