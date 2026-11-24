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
  let text = "# <name> <WebSocket URL> [<Origin>|-]\n\n\
              destiny.gg wss://chat.destiny.gg/ws https://www.destiny.gg\r\n\
              chat.example ws://localhost:9000/ws -  # local\n\
              other_2\twss://other.example/ws\n";
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
fn says_whats_wrong_with_a_line() {
  let url = "wss://a.example/ws";
  for (text, why) in [
    ("a\n".to_owned(), "line 1: not `<name> <URL>"),
    (format!("a {url} - x\n"), "line 1: not `<name> <URL>"),
    (format!("# ok\nA {url}\n"), "line 2: a name is lowercase"),
    (format!("../a {url}\n"), "line 1: a name is lowercase"),
    (format!(".a {url}\n"), "line 1: a name is lowercase"),
    (format!("a. {url}\n"), "line 1: a name is lowercase"),
    (format!("a..b {url}\n"), "line 1: a name is lowercase"),
    (format!("live {url}\n"), "line 1: a name is lowercase"),
    (format!("backfill {url}\n"), "line 1: a name is lowercase"),
    ("a https://a.example\n".to_owned(), "line 1: a URL starts"),
    (format!("a {url} a.example\n"), "line 1: an Origin starts"),
    (
      format!("a {url}\nb {url}\na {url}\n"),
      "line 3: listed twice",
    ),
    ("# none\n\n".to_owned(), "no channels listed"),
  ] {
    let e = channels::parse(&text).unwrap_err();
    assert!(e.starts_with(why), "{text:?}: {e}");
  }
}

#[test]
fn writes_a_line_as_read() {
  for channel in [
    Channel::destiny(),
    channel("a.example", "wss://a.example/ws", None),
  ] {
    let read = channels::parse(&channel.line()).unwrap();
    assert_eq!(read, [channel]);
  }
  assert_eq!(
    Channel::destiny().line(),
    "destiny.gg wss://chat.destiny.gg/ws https://www.destiny.gg"
  );
}

#[test]
fn is_destiny_gg_without_a_file() {
  let root = TempDir::new("logger-dgg-channels");
  let path = root.path().join(channels::FILE);
  assert_eq!(channels::load(&path).unwrap(), [Channel::destiny()]);

  fs::write(&path, "a.example wss://a.example/ws\n").unwrap();
  assert_eq!(
    channels::load(&path).unwrap(),
    [channel("a.example", "wss://a.example/ws", None)]
  );

  fs::write(&path, "\n").unwrap();
  let e = channels::load(&path).unwrap_err();
  assert!(e.contains("no channels listed"), "{e}");
}
