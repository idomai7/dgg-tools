use ::dgg_api::chat::protocol::event::{
  ErrorCode, Event, Subscription, User, Watching,
};

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
      (Event::Other { .. }, "ME") => {}
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
      id: Some(1011),
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
  assert_eq!(broadcast.user.id, Some(-1));
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

/// Frames shaped as chat-gui reads them, for the events it types that
/// weren't seen: names and text are placeholders.
const CHAT_GUI: &str = include_str!("fixtures/chat-gui.txt");

fn chat_gui(event: &str) -> Vec<Event> {
  CHAT_GUI
    .lines()
    .filter(|line| line.split(' ').next() == Some(event))
    .map(Event::parse)
    .collect()
}

#[test]
fn decodes_every_chat_gui_fixture() {
  for line in CHAT_GUI.lines() {
    let event = Event::parse(line);
    assert_eq!(event.name(), line.split(' ').next().unwrap());
    assert!(
      !matches!(event, Event::Other { .. } | Event::Unreadable { .. }),
      "{line}: {event:?}"
    );
  }
}

#[test]
fn replies() {
  let errors = chat_gui("ERR");
  let Event::Err(throttled) = &errors[0] else {
    panic!()
  };
  assert_eq!(throttled.description, ErrorCode::Throttled);
  assert!(throttled.description.rejects_message());
  let Event::Err(muted) = &errors[1] else {
    panic!()
  };
  assert_eq!(
    (&muted.description, muted.mute_time_left),
    (&ErrorCode::Muted, Some(540))
  );
  let Event::Err(new) = Event::parse(r#"ERR {"description":"newcode"}"#) else {
    panic!()
  };
  assert_eq!(new.description, ErrorCode::Other("newcode".into()));
  assert!(!new.description.rejects_message());

  let Event::PrivMsg(whisper) = &chat_gui("PRIVMSG")[0] else {
    panic!()
  };
  assert_eq!(
    (whisper.nick.as_str(), whisper.message_id),
    ("user1", Some(42))
  );
  assert_eq!(chat_gui("PRIVMSGSENT"), [Event::PrivMsgSent]);
}

#[test]
fn moderation_from_chat_gui() {
  let Event::Ban(ban) = &chat_gui("BAN")[0] else {
    panic!()
  };
  assert_eq!((ban.nick.as_str(), ban.target.as_str()), ("user1", "user2"));
  let Event::Unmute(unmute) = &chat_gui("UNMUTE")[0] else {
    panic!()
  };
  assert_eq!(unmute.nick, "System");
  let tiers: Vec<_> = chat_gui("SUBONLY")
    .iter()
    .map(|event| match event {
      Event::SubOnly(mode) => mode.tier(),
      _ => panic!(),
    })
    .collect();
  assert_eq!(tiers, [Some(3), None]);
  let pins = chat_gui("PIN");
  let Event::Pin(unpin) = &pins[1] else {
    panic!()
  };
  assert!(unpin.data.is_empty());
  let Event::AddPhrase(phrase) = &chat_gui("ADDPHRASE")[0] else {
    panic!()
  };
  assert_eq!(phrase.phrase, "phrase 1");
}

#[test]
fn paid_from_chat_gui() {
  let Event::GiftSub(gift) = &chat_gui("GIFTSUB")[0] else {
    panic!()
  };
  assert_eq!(
    (
      gift.user.nick.as_str(),
      gift.recipient.nick.as_str(),
      gift.tier
    ),
    ("user1", "user2", 2)
  );
  let Event::MassGift(mass) = &chat_gui("MASSGIFT")[0] else {
    panic!()
  };
  assert_eq!(mass.quantity, 5);
  let Event::Donation(donation) = &chat_gui("DONATION")[0] else {
    panic!()
  };
  assert_eq!((donation.amount, donation.audio.as_deref()), (500, None));
}

#[test]
fn polls_from_chat_gui() {
  let Event::PollStart(poll) = &chat_gui("POLLSTART")[0] else {
    panic!()
  };
  assert_eq!(
    (poll.options.len(), poll.total_votes, poll.time, poll.hidden),
    (2, 7, 30_000, false)
  );
  let stops = chat_gui("POLLSTOP");
  let Event::PollStop(hidden) = &stops[1] else {
    panic!()
  };
  assert_eq!(hidden.totals.as_deref(), Some(&[5, 4][..]));
  let casts: Vec<_> = chat_gui("VOTECAST")
    .iter()
    .map(|event| match event {
      Event::VoteCast(cast) => (cast.vote, cast.quantity),
      _ => panic!(),
    })
    .collect();
  assert_eq!(casts, [(Some(2), 1), (None, 3)]);
  // Sent as a string, counted as a number.
  let Event::VoteCounted(counted) = &chat_gui("VOTECOUNTED")[0] else {
    panic!()
  };
  assert_eq!(counted.vote, Some(2));
}

#[test]
fn spotlights_from_chat_gui() {
  let Event::Spotlight(spotlight) = &chat_gui("SPOTLIGHT")[0] else {
    panic!()
  };
  assert_eq!(
    (
      spotlight.key.as_str(),
      spotlight.user.nick.as_str(),
      spotlight.spotlighted_by.as_str()
    ),
    ("key1", "user1", "user2")
  );
  assert_eq!(
    chat_gui("SPOTLIGHTS"),
    [Event::Spotlights(vec![spotlight.clone()])]
  );
  // As seen: empty.
  assert_eq!(Event::parse("SPOTLIGHTS []"), Event::Spotlights(vec![]));
  let Event::Unspotlight(ended) = &chat_gui("UNSPOTLIGHT")[0] else {
    panic!()
  };
  assert_eq!(ended.key, "key1");
}
