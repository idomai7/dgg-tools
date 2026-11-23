//! The Kick channel list. The channels and ids are made up.

use {
  ::common::temp_dir::TempDir,
  ::kick_api::channel,
  ::logger::kick::channels::{self, Channel},
  ::std::fs,
};

fn channel(slug: &str, channel_id: u64, chatroom_id: u64) -> Channel {
  Channel {
    slug: slug.into(),
    channel_id,
    chatroom_id,
  }
}

fn found(slug: &str) -> Result<channel::Channel, String> {
  channel::parse(&format!(
    r#"{{"id":789,"user_id":456,"slug":"{slug}","chatroom":{{"id":123}},
        "followers_count":5}}"#
  ))
  .map_err(|e| e.to_string())
}

#[test]
fn reads_a_list() {
  let text = "# <slug> <channel id> <chatroom id>\n\n\
              some-channel 789 123  # the first\r\nother_2\t10 20\n";
  assert_eq!(
    channels::parse(text).unwrap(),
    [
      channel("some-channel", 789, 123),
      channel("other_2", 10, 20)
    ]
  );
  assert_eq!(channels::parse("").unwrap(), []);
}

#[test]
fn says_whats_wrong_with_a_line() {
  for (text, why) in [
    ("a 1\n", "line 1: not `<slug> <channel id> <chatroom id>`"),
    ("a 1 2 3\n", "line 1: not `<slug>"),
    ("# ok\nSome 1 2\n", "line 2: a slug is lowercase"),
    ("../a 1 2\n", "line 1: a slug is lowercase"),
    ("a 1 x\n", "line 1: an id is a number"),
    ("a 1 2\nb 3 4\na 5 6\n", "line 3: listed twice"),
  ] {
    let e = channels::parse(text).unwrap_err();
    assert!(e.starts_with(why), "{text:?}: {e}");
  }
}

#[test]
fn records_a_channel_in_place() {
  let text = "# channels\r\na 1 2 # kept\r\nb 3 4\r\n";
  assert_eq!(
    channels::record(text, &channel("a", 5, 6)).unwrap(),
    "# channels\r\na 5 6 # kept\r\nb 3 4\r\n"
  );
  assert_eq!(
    channels::record(text, &channel("c", 7, 8)).unwrap(),
    "# channels\r\na 1 2 # kept\r\nb 3 4\r\nc 7 8\n"
  );
  assert_eq!(
    channels::record("b 3 4", &channel("c", 7, 8)).unwrap(),
    "b 3 4\nc 7 8\n"
  );
  assert_eq!(
    channels::record("", &channel("c", 7, 8)).unwrap(),
    "c 7 8\n"
  );
  // A broken list is left for its owner to fix.
  assert!(channels::record("a b\n", &channel("c", 7, 8)).is_err());
}

#[test]
fn updates_the_file() {
  let root = TempDir::new("logger-kick-channels");
  let path = root.path().join(channels::FILE);
  assert_eq!(channels::load(&path).unwrap(), []);

  channels::update(&path, &channel("a", 1, 2)).unwrap();
  channels::update(&path, &channel("b", 3, 4)).unwrap();
  channels::update(&path, &channel("a", 5, 6)).unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), "a 5 6\nb 3 4\n");
  assert_eq!(
    channels::load(&path).unwrap(),
    [channel("a", 5, 6), channel("b", 3, 4)]
  );

  fs::write(&path, "a 5\n").unwrap();
  let e = channels::update(&path, &channel("b", 3, 4)).unwrap_err();
  assert!(e.contains("line 1"), "{e}");
  assert_eq!(fs::read_to_string(&path).unwrap(), "a 5\n");
}

#[test]
fn looks_up_a_channel() {
  let mut asked = None;
  let looked_up = channels::look_up(" Some-Channel ", |slug| {
    asked = Some(slug.to_owned());
    found(slug)
  });
  assert_eq!(looked_up.unwrap(), channel("some-channel", 789, 123));
  assert_eq!(asked.as_deref(), Some("some-channel"));

  let e = channels::look_up("a/b", |_| panic!("not asked")).unwrap_err();
  assert!(e.contains("a slug"), "{e}");
  let e = channels::look_up("a", |_| found("b")).unwrap_err();
  assert_eq!(e, r#"the lookup answered for "b""#);
  let e = channels::look_up("a", |_| Err("curl: timed out".into()));
  assert_eq!(e.unwrap_err(), "curl: timed out");
}
