use ::dgg_api::chat::protocol::event::{Event, Subscription, User, Watching};

/// Frames dgg sent, anonymised: a few of each event seen.
const FRAMES: &str = include_str!("fixtures/frames.txt");

fn fixture(event: &str) -> Vec<Event> {
  FRAMES
    .lines()
    .filter(|line| line.split(' ').next() == Some(event))
    .map(Event::parse)
    .collect()
}

#[test]
fn decodes_every_fixture() {
  for line in FRAMES.lines() {
    let event = Event::parse(line);
    let name = line.split(' ').next().unwrap();
    assert_eq!(event.name(), name);
    match (&event, name) {
      (Event::Other { .. }, "ME" | "SPOTLIGHTS") => {}
      (Event::Other { .. } | Event::Unreadable { .. }, _) => {
        panic!("{line}: {event:?}")
      }
      _ => {}
    }
  }
}

#[test]
fn messages() {
  let Event::Msg(message) = &fixture("MSG")[0] else {
    panic!()
  };
  assert_eq!(
    message.user,
    User {
      id: 1011,
      nick: "user13".into(),
      roles: vec![],
      features: vec!["flair13".into(), "subscriber".into()],
      created_date: Some("2020-01-01T00:00:00Z".into()),
      watching: Some(Watching {
        platform: "kick".into(),
        id: "channel".into(),
      }),
      subscription: Some(Subscription {
        tier: 1,
        source: "destiny.gg".into(),
      }),
      patreon_member: false,
      photo: Some("buddy".into()),
    }
  );
  assert_eq!(
    (message.timestamp, message.data.as_str()),
    (1794203923846, "message 6")
  );
  let Event::Msg(moderator) = &fixture("MSG")[1] else {
    panic!()
  };
  assert_eq!(moderator.user.roles[0], "MODERATOR");
  assert_eq!(moderator.user.subscription, None);
}

#[test]
fn history_holds_frames() {
  let histories = fixture("HISTORY");
  let Event::History(frames) = &histories[1] else {
    panic!()
  };
  let names: Vec<_> = frames.iter().map(Event::name).collect();
  assert_eq!(names, ["XPOST", "MSG", "MSG"]);
  let Event::XPost(post) = &frames[0] else {
    panic!()
  };
  assert_eq!(post.url, "https://x.com/user7/status/1");
  let paid = fixture("PAIDEVENTS");
  assert_eq!(paid[0], Event::PaidEvents(vec![]));
}

#[test]
fn moderation() {
  let Event::Mute(mute) = &fixture("MUTE")[0] else {
    panic!()
  };
  assert_eq!(
    (mute.by.nick.as_str(), mute.target.as_str(), mute.duration),
    ("user16", "user17", 14400)
  );
  let Event::Death(death) = &fixture("DEATH")[0] else {
    panic!()
  };
  assert_eq!((death.duration, death.reason.as_str()), (600, "reason 1"));
}

#[test]
fn names_and_presence() {
  let Event::Names(names) = &fixture("NAMES")[0] else {
    panic!()
  };
  assert_eq!((names.users.len(), names.connection_count), (3, 2364));
  let Event::Quit(quit) = &fixture("QUIT")[0] else {
    panic!()
  };
  assert_eq!(quit.user.nick, "user10");
  assert!(matches!(&fixture("JOIN")[0], Event::Join(_)));
  assert!(matches!(&fixture("UPDATEUSER")[0], Event::UpdateUser(_)));
}

#[test]
fn paid_and_system() {
  let Event::Subscription(sub) = &fixture("SUBSCRIPTION")[0] else {
    panic!()
  };
  assert_eq!((sub.amount, sub.tier, sub.streak), (500, 1, 2));
  assert_eq!(sub.user.nick, sub.nick);
  let Event::Broadcast(broadcast) = &fixture("BROADCAST")[0] else {
    panic!()
  };
  // System's user leaves most fields out.
  assert_eq!(broadcast.user.id, -1);
  assert_eq!(broadcast.user.photo, None);
  assert!(!broadcast.user.patreon_member);
}

#[test]
fn keeps_what_it_cannot_type() {
  assert_eq!(
    Event::parse("PING {\"data\":1}"),
    Event::Other {
      event: "PING".into(),
      payload: "{\"data\":1}".into(),
    }
  );
  let Event::Unreadable { event, payload, .. } = Event::parse("MSG {}") else {
    panic!()
  };
  assert_eq!((event.as_str(), payload.as_str()), ("MSG", "{}"));
  // A bad frame inside HISTORY spoils only itself.
  let Event::History(frames) = Event::parse("HISTORY [\"MSG x\",\"ME null\"]")
  else {
    panic!()
  };
  assert!(matches!(frames[0], Event::Unreadable { .. }));
  assert!(matches!(frames[1], Event::Other { .. }));
}
