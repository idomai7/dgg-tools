use {
  ::kick_api::realtime::{
    centrifugo::{self, push, reply},
    event::{
      Deleted, Event, MessageDeleted, PinnedMessageDeleted, User, UserBanned,
    },
  },
  ::serde_json::json,
};

/// Frames Kick's Centrifugo sent the probe (2026-11-19), anonymised: one
/// push of each event seen, among others.
const FRAMES: &str = include_str!("fixtures/frames.txt");

/// Each event in the fixtures, in order.
fn events() -> Vec<Event> {
  FRAMES
    .lines()
    .filter_map(|frame| {
      let reply = centrifugo::replies(frame).next()?.unwrap();
      let Some(reply::Body::Push(push)) = reply.body else {
        return None;
      };
      let Some(push::Body::Publication(publication)) = push.body else {
        panic!("{frame}");
      };
      Some(Event::from_publication(&publication.data).unwrap())
    })
    .collect()
}

fn user(id: u64, name: &str) -> User {
  User {
    id,
    username: name.into(),
    slug: Some(name.to_lowercase()),
    identity: None,
    rest: None,
  }
}

#[test]
fn decodes_every_fixture_complete() {
  let events = events();
  let names: Vec<_> = events.iter().map(Event::name).collect();
  assert_eq!(
    names,
    [
      "App\\Events\\ChatMessageEvent",
      "App\\Events\\MessageDeletedEvent",
      "App\\Events\\PinnedMessageCreatedEvent",
      "App\\Events\\PinnedMessageDeletedEvent",
      "App\\Events\\UserBannedEvent",
      "App\\Events\\UserUnbannedEvent",
      "App\\Events\\UserBannedEvent",
      "ChatSettingsChanged",
      "App\\Events\\ChatroomUpdatedEvent",
      "App\\Events\\ChatroomClearEvent",
    ]
  );
  for event in &events {
    assert!(event.complete(), "{event:#?}");
  }
}

#[test]
fn chat_message() {
  let Event::ChatMessageEvent(message) = &events()[0] else {
    panic!();
  };
  assert_eq!(message.id, "00000000-0000-4000-8000-000000000002");
  assert_eq!(message.chatroom_id, Some(1000));
  assert_eq!(message.content, "probe 1 plain");
  assert_eq!(message.r#type.as_deref(), Some("message"));
  // 2026-11-19T12:01:56+00:00
  assert_eq!(message.created_at, Some(1_795_089_716_000));
  assert_eq!(
    (message.sender.id, &*message.sender.username),
    (3000, "channel")
  );
  let identity = message.sender.identity.as_ref().unwrap();
  assert_eq!(identity.color.as_deref(), Some("#00FF00"));
  assert_eq!(identity.badges[0].r#type, "broadcaster");
  assert_eq!(identity.badges_v2.as_ref().unwrap()[0].name, "level");
  let metadata = message.metadata.as_ref().unwrap();
  assert_eq!(metadata.message_ref.as_deref(), Some("1795089716345"));
}

#[test]
fn message_ref_is_opaque() {
  let message = |metadata: serde_json::Value| {
    let data = json!({
      "id": "m", "content": "hi",
      "sender": { "id": 1, "username": "user1" },
      "metadata": metadata,
    });
    let event =
      Event::parse("App\\Events\\ChatMessageEvent", &data.to_string());
    let Event::ChatMessageEvent(message) = event else {
      panic!("{event:?}");
    };
    message.metadata.map(|metadata| metadata.message_ref)
  };
  assert_eq!(
    message(json!({ "message_ref": "not-a-time" })),
    Some(Some("not-a-time".into()))
  );
  assert_eq!(message(json!({ "message_ref": "" })), Some(Some("".into())));
  assert_eq!(message(json!(null)), None);
}

#[test]
fn deleted_and_banned() {
  let events = events();
  assert_eq!(
    events[1],
    Event::MessageDeletedEvent(MessageDeleted {
      id: Some("00000000-0000-4000-8000-000000000004".into()),
      message: Deleted {
        id: "00000000-0000-4000-8000-000000000005".into(),
        rest: None,
      },
      ai_moderated: Some(false),
      violated_rules: Some(vec![]),
      rest: None,
    })
  );
  assert_eq!(
    events[4],
    Event::UserBannedEvent(UserBanned {
      id: Some("00000000-0000-4000-8000-000000000007".into()),
      user: user(1001, "user1"),
      banned_by: Some(user(0, "channel")),
      permanent: false,
      duration: Some(1),
      // 2026-11-19T12:06:45+00:00
      expires_at: Some(1_795_090_005_000),
      rest: None,
    })
  );
  let Event::UserBannedEvent(ban) = &events[6] else {
    panic!();
  };
  assert!(ban.permanent);
  assert_eq!((ban.duration, ban.expires_at), (None, None));
}

#[test]
fn pins() {
  let events = events();
  let Event::PinnedMessageCreatedEvent(pin) = &events[2] else {
    panic!();
  };
  assert_eq!(pin.message.content, "probe 8 pin me");
  assert_eq!(pin.message.thread_parent_id.as_deref(), Some(""));
  assert_eq!(pin.duration.as_deref(), Some("1200"));
  assert_eq!(pin.pinned_by.as_ref().unwrap().id, 3000);
  assert_eq!(
    events[3],
    Event::PinnedMessageDeletedEvent(PinnedMessageDeleted { rest: None })
  );
  let unpin =
    |data| Event::parse("App\\Events\\PinnedMessageDeletedEvent", data);
  let Event::PinnedMessageDeletedEvent(unpinned) = unpin(r#"{"id":"u"}"#)
  else {
    panic!();
  };
  assert_eq!(unpinned.rest.unwrap()["id"], "u");
  assert!(unpin("{}").complete());
  assert!(matches!(unpin("[1]"), Event::Unreadable { .. }));
}

#[test]
fn settings() {
  let events = events();
  let Event::ChatSettingsChanged(changed) = &events[7] else {
    panic!();
  };
  assert_eq!(changed.channel.as_ref().unwrap().id, 2000);
  // 2026-11-19T12:07:30Z
  assert_eq!(changed.created_at, Some(1_795_090_050_000));
  let slow = changed.settings.slow_mode.as_ref().unwrap();
  assert_eq!((slow.enabled, slow.duration_seconds), (true, Some(120)));
  assert!(!changed.settings.allow_links.as_ref().unwrap().enabled);
  let Event::ChatroomUpdatedEvent(updated) = &events[8] else {
    panic!();
  };
  assert_eq!(updated.id, 1000);
  let slow = updated.slow_mode.as_ref().unwrap();
  assert_eq!((slow.enabled, slow.message_interval), (true, Some(120)));
}

#[test]
fn keeps_unnamed_fields() {
  let data = json!({
    "id": "m", "content": "hi", "new": 1,
    "sender": {
      "id": 1, "username": "user1",
      "identity": { "badges": [{ "type": "subscriber", "count": 3 }] },
    },
  });
  let event = Event::parse("App\\Events\\ChatMessageEvent", &data.to_string());
  assert!(!event.complete());
  let Event::ChatMessageEvent(message) = event else {
    panic!();
  };
  assert_eq!(message.rest.unwrap()["new"], 1);
  let badge = &message.sender.identity.unwrap().badges[0];
  assert_eq!(badge.rest.as_ref().unwrap()["count"], 3);
}

#[test]
fn keeps_unreadable_timestamps() {
  let message = |created_at: serde_json::Value| {
    let data = json!({
      "id": "m", "content": "hi",
      "sender": { "id": 1, "username": "user1" },
      "created_at": created_at,
    });
    let event =
      Event::parse("App\\Events\\ChatMessageEvent", &data.to_string());
    let complete = event.complete();
    let Event::ChatMessageEvent(message) = event else {
      panic!("{event:?}");
    };
    (message.created_at, message.rest, complete)
  };
  assert_eq!(
    message(json!("1970-01-01T00:00:01Z")),
    (Some(1000), None, true)
  );
  assert_eq!(message(json!(null)), (None, None, true));
  let (created_at, rest, complete) = message(json!("yesterday"));
  assert_eq!((created_at, complete), (None, false));
  assert_eq!(rest.unwrap()["created_at"], "yesterday");
  let (_, rest, _) = message(json!(1795089716));
  assert_eq!(rest.unwrap()["created_at"], 1795089716);
}

#[test]
fn other_and_unreadable() {
  assert_eq!(
    Event::parse("App\\Events\\GiftedSubscriptionsEvent", "{}"),
    Event::Other {
      event: "App\\Events\\GiftedSubscriptionsEvent".into(),
      data: "{}".into(),
    }
  );
  // Without its prefix, it isn't the event Kick was seen sending.
  assert!(matches!(
    Event::parse("ChatMessageEvent", "{}"),
    Event::Other { .. }
  ));
  let event = Event::parse("App\\Events\\UserBannedEvent", r#"{"id":"b"}"#);
  let Event::Unreadable {
    event: name,
    data,
    error,
  } = &event
  else {
    panic!("{event:?}");
  };
  assert_eq!(
    (&**name, &**data),
    ("App\\Events\\UserBannedEvent", r#"{"id":"b"}"#)
  );
  assert!(error.contains("user"), "{error}");
  assert_eq!(event.name(), "App\\Events\\UserBannedEvent");
  assert!(!event.complete());
}

#[test]
fn from_publication_needs_kicks_shape() {
  assert_eq!(Event::from_publication(&json!({ "event": "e" })), None);
  assert_eq!(
    Event::from_publication(&json!({ "event": "e", "data": {} })),
    None
  );
  assert_eq!(
    Event::from_publication(&json!({ "event": "e", "data": "{}" })),
    Some(Event::Other {
      event: "e".into(),
      data: "{}".into()
    })
  );
}
