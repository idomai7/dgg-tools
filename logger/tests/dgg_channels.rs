//! The dgg channel list. The channels besides destiny.gg are made up.

use {
  ::common::temp_dir::TempDir,
  ::logger::dgg::channels::{self, Channel},
  ::std::fs,
};

fn channel(name: &str, url: &str, origin: Option<&str>) -> Channel {
  Channel {
    name: name.into(),
    url: url.into(),
    origin: origin.map(Into::into),
  }
}

#[test]
fn reads_a_list() {
  let text = r#"// the channels
    {
      "channels": [
        {
          "name": "destiny.gg",
          "url": "wss://chat.destiny.gg/ws",
          "origin": "https://www.destiny.gg",
        },
        { "name": "chat.example", "url": "ws://localhost:9000/ws",
          "origin": null }, // local
        { "name": "other_2", "url": "wss://other.example/ws" },
      ],
    }"#;
  assert_eq!(
    channels::parse(text).unwrap(),
    [
      Channel::destiny(),
      channel("chat.example", "ws://localhost:9000/ws", None),
      channel("other_2", "wss://other.example/ws", None),
    ]
  );
}

#[test]
fn says_whats_wrong_with_a_channel() {
  let list = |entries: &str| format!(r#"{{ "channels": [{entries}] }}"#);
  let named =
    |name: &str| format!(r#"{{ "name": "{name}", "url": "wss://a/ws" }}"#);
  let ok = named("a");
  for (text, why) in [
    (list(&named("A")), "channel 1: a name is lowercase"),
    (
      list(&format!("{ok}, {}", named("../a"))),
      "channel 2: a name is",
    ),
    (list(&named(".a")), "channel 1: a name is lowercase"),
    (list(&named("a.")), "channel 1: a name is lowercase"),
    (list(&named("a..b")), "channel 1: a name is lowercase"),
    (list(&named("live")), "channel 1: a name is lowercase"),
    (list(&named("backfill")), "channel 1: a name is lowercase"),
    (
      list(r#"{ "name": "a", "url": "https://a" }"#),
      "channel 1: a URL starts",
    ),
    (
      list(r#"{ "name": "a", "url": "wss://a", "origin": "a.example" }"#),
      "channel 1: an Origin starts",
    ),
    (
      list(&format!("{ok}, {}, {ok}", named("b"))),
      "channel 3: a listed twice",
    ),
    (list(""), "no channels listed"),
    (list(r#"{ "name": "a" }"#), "missing field `url`"),
    (
      list(r#"{ "name": "a", "url": "wss://a", "x": 1 }"#),
      "unknown field `x`",
    ),
    ("{}".into(), "missing field `channels`"),
  ] {
    let e = channels::parse(&text).unwrap_err();
    assert!(e.starts_with(why), "{text:?}: {e}");
  }
}

#[test]
fn writes_a_list_as_read() {
  let all = [
    Channel::destiny(),
    channel("a.example", "wss://a.example/ws", None),
  ];
  let text = channels::render(&all);
  assert_eq!(channels::parse(&text).unwrap(), all);
  assert!(text.contains(
    "    {\n      \"name\": \"destiny.gg\",\n      \"url\": \
     \"wss://chat.destiny.gg/ws\",\n      \"origin\": \
     \"https://www.destiny.gg\",\n    },\n"
  ));
  assert!(text.contains("\"origin\": null,"));
}

#[test]
fn converts_a_plain_list() {
  let text = "# <name> <WebSocket URL> [<Origin>|-]\n\n\
              destiny.gg wss://chat.destiny.gg/ws https://www.destiny.gg\r\n\
              chat.example ws://localhost:9000/ws -  # local\n\
              other_2\twss://other.example/ws\n";
  let all = [
    Channel::destiny(),
    channel("chat.example", "ws://localhost:9000/ws", None),
    channel("other_2", "wss://other.example/ws", None),
  ];
  assert_eq!(channels::parse_plain(text).unwrap(), all);
  let converted = channels::convert_plain(text).unwrap();
  assert_eq!(channels::parse(&converted).unwrap(), all);
  assert_eq!(converted, channels::render(&all));
}

#[test]
fn says_whats_wrong_with_a_plain_line() {
  let url = "wss://a.example/ws";
  for (text, why) in [
    ("a\n".to_owned(), "line 1: not `<name> <URL>"),
    (format!("a {url} - x\n"), "line 1: not `<name> <URL>"),
    (format!("# ok\nA {url}\n"), "line 2: a name is lowercase"),
    (format!("live {url}\n"), "line 1: a name is lowercase"),
    ("a https://a.example\n".to_owned(), "line 1: a URL starts"),
    (format!("a {url} a.example\n"), "line 1: an Origin starts"),
    (
      format!("a {url}\nb {url}\na {url}\n"),
      "line 3: listed twice",
    ),
    ("# none\n\n".to_owned(), "no channels listed"),
  ] {
    let e = channels::parse_plain(&text).unwrap_err();
    assert!(e.starts_with(why), "{text:?}: {e}");
  }
  let e = channels::parse_plain(&format!("a {url} a.example\n")).unwrap_err();
  assert!(e.contains("or is -"), "{e}");
}

#[test]
fn is_destiny_gg_without_a_file() {
  let root = TempDir::new("logger-dgg-channels");
  let path = root.path().join(channels::FILE);
  assert_eq!(channels::load(&path).unwrap(), [Channel::destiny()]);

  let one = [channel("a.example", "wss://a.example/ws", None)];
  fs::write(&path, channels::render(&one)).unwrap();
  assert_eq!(channels::load(&path).unwrap(), one);

  fs::write(&path, channels::render(&[])).unwrap();
  let e = channels::load(&path).unwrap_err();
  assert!(e.contains("no channels listed"), "{e}");
}
