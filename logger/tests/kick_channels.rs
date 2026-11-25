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
  let text = r#"// the channels
    {
      "channels": [
        { "slug": "some-channel", "channel_id": 789, "chatroom_id": 123 },
        /* the second */ { "slug": "other_2", "channel_id": 10,
          "chatroom_id": 20 },
      ],
    }"#;
  assert_eq!(
    channels::parse(text).unwrap(),
    [
      channel("some-channel", 789, 123),
      channel("other_2", 10, 20)
    ]
  );
  assert_eq!(channels::parse(&channels::render(&[])).unwrap(), []);
  let bot = [channel("@bot", 1, 2)];
  assert_eq!(channels::parse(&channels::render(&bot)).unwrap(), bot);
}

#[test]
fn says_whats_wrong_with_a_channel() {
  let list = |entries: &str| format!(r#"{{ "channels": [{entries}] }}"#);
  let entry = |slug: &str| {
    format!(r#"{{ "slug": "{slug}", "channel_id": 1, "chatroom_id": 2 }}"#)
  };
  for (text, why) in [
    (list(&entry("Some")), "channel 1: a slug is lowercase"),
    (
      list(&format!("{}, {}", entry("a"), entry("../a"))),
      "channel 2: a slug is lowercase",
    ),
    (
      list(&format!("{}, {}, {}", entry("a"), entry("b"), entry("a"))),
      "channel 3: a listed twice",
    ),
    (
      list(r#"{ "slug": "a", "channel_id": 1 }"#),
      "missing field `chatroom_id`",
    ),
    (
      list(r#"{ "slug": "a", "channel_id": "x", "chatroom_id": 2 }"#),
      "a value of the wrong type",
    ),
  ] {
    let e = channels::parse(&text).unwrap_err();
    assert!(e.starts_with(why), "{text:?}: {e}");
  }
}

#[test]
fn converts_a_plain_list() {
  let text = "# <slug> <channel id> <chatroom id>\n\n\
              some-channel 789 123  # the first\r\nother_2\t10 20\n";
  let all = [
    channel("some-channel", 789, 123),
    channel("other_2", 10, 20),
  ];
  assert_eq!(channels::parse_plain(text).unwrap(), all);
  let converted = channels::convert_plain(text).unwrap();
  assert_eq!(converted, channels::render(&all));
  assert_eq!(channels::parse(&converted).unwrap(), all);
  assert_eq!(channels::convert_plain("").unwrap(), channels::render(&[]));
  for (text, why) in [
    ("a 1\n", "line 1: not `<slug> <channel id> <chatroom id>`"),
    ("a 1 2 3\n", "line 1: not `<slug>"),
    ("# ok\nSome 1 2\n", "line 2: a slug is lowercase"),
    ("a 1 x\n", "line 1: an id is a number"),
    ("a 1 2\nb 3 4\na 5 6\n", "line 3: listed twice"),
  ] {
    let e = channels::convert_plain(text).unwrap_err();
    assert!(e.starts_with(why), "{text:?}: {e}");
  }
}

#[test]
fn records_a_channel_in_place() {
  let text = "// channels\r\n{\r\n  \"channels\": [\r\n    \
              { \"slug\": \"a\", \"channel_id\": 1, \"chatroom_id\": 2 }, \
              // kept\r\n    \
              { \"slug\": \"b\", \"channel_id\": 3, \"chatroom_id\": 4 }\r\n  \
              ],\r\n}\r\n";
  let entry = |slug: &str, ids: (u64, u64)| channel(slug, ids.0, ids.1).json();
  // Its entry replaced, the comment after it kept.
  assert_eq!(
    channels::record(text, &channel("a", 5, 6)).unwrap(),
    text.replace(&entry("a", (1, 2)), &entry("a", (5, 6)))
  );
  // A new one at the end, the last given its comma.
  let added = channels::record(text, &channel("c", 7, 8)).unwrap();
  assert_eq!(
    added,
    text.replace(
      &format!("{}\r\n", entry("b", (3, 4))),
      &format!("{},\r\n    {},\r\n", entry("b", (3, 4)), entry("c", (7, 8)))
    )
  );
  // Into an empty array, on a line of the key's, or of its own.
  let empty = r#"{ "channels": [] }"#;
  assert_eq!(
    channels::record(empty, &channel("c", 7, 8)).unwrap(),
    format!("{{ \"channels\": [\n  {},\n] }}", entry("c", (7, 8)))
  );
  let template = channels::render(&[]);
  let recorded = channels::record(&template, &channel("c", 7, 8)).unwrap();
  assert_eq!(
    recorded,
    channels::render(&[])
      .replace("  ],", &format!("    {},\n  ],", entry("c", (7, 8))))
  );
  assert_eq!(channels::parse(&recorded).unwrap(), [channel("c", 7, 8)]);
  // A broken list is left for its owner to fix.
  assert!(
    channels::record("{ \"channels\": [1] }", &channel("c", 7, 8)).is_err()
  );
}

#[test]
fn updates_the_file() {
  let root = TempDir::new("logger-kick-channels");
  let path = root.path().join(channels::FILE);
  assert_eq!(channels::load(&path).unwrap(), []);

  channels::update(&path, &channel("a", 1, 2)).unwrap();
  channels::update(&path, &channel("b", 3, 4)).unwrap();
  channels::update(&path, &channel("a", 5, 6)).unwrap();
  let all = [channel("a", 5, 6), channel("b", 3, 4)];
  assert_eq!(channels::load(&path).unwrap(), all);
  // As if written whole, the example kept.
  let written = fs::read_to_string(&path).unwrap();
  assert!(
    written.contains("// { \"slug\": \"somechannel\""),
    "{written}"
  );
  assert!(
    written.contains(&format!("    {},\n", all[1].json())),
    "{written}"
  );

  fs::write(&path, "{ \"channels\": [ { \"slug\": \"a\" } ] }").unwrap();
  let e = channels::update(&path, &channel("b", 3, 4)).unwrap_err();
  assert!(e.contains("missing field"), "{e}");
  assert_eq!(
    fs::read_to_string(&path).unwrap(),
    "{ \"channels\": [ { \"slug\": \"a\" } ] }"
  );
}

#[::tokio::test]
async fn looks_up_a_channel() {
  let mut asked = None;
  let looked_up = channels::look_up(" Some-Channel ", async |slug: &str| {
    asked = Some(slug.to_owned());
    found(slug)
  });
  let looked_up = looked_up.await;
  assert_eq!(looked_up.unwrap(), channel("some-channel", 789, 123));
  assert_eq!(asked.as_deref(), Some("some-channel"));
  // An app's bot user's slug starts with @.
  let bot = channels::look_up("@Some-Bot", async |slug: &str| found(slug));
  assert_eq!(bot.await.unwrap(), channel("@some-bot", 789, 123));

  let not_asked = async |_: &str| panic!("not asked");
  let e = channels::look_up("a/b", not_asked).await.unwrap_err();
  assert!(e.contains("a slug"), "{e}");
  let e = channels::look_up("a", async |_: &str| found("b")).await;
  assert_eq!(e.unwrap_err(), r#"the lookup answered for "b""#);
  let timed_out = async |_: &str| Err("timed out".into());
  let e = channels::look_up("a", timed_out).await;
  assert_eq!(e.unwrap_err(), "timed out");
}
