use ::journal::{
  DecodeError,
  record::v0::{Body, Record},
};

#[test]
fn line_format() {
  let line = |body| {
    let record = Record {
      receive_time: 5,
      instance: "a".into(),
      connection: 2,
      body,
    };
    let mut out = Vec::new();
    record.encode(&mut out);
    let encoded = String::from_utf8(out).unwrap();
    let encoded = encoded.strip_suffix('\n').unwrap().to_owned();
    assert_eq!(Record::decode(encoded.as_bytes()), Ok(record));
    encoded
  };
  assert_eq!(
    line(Body::Frame(r#"MSG {"data":"hi \"x\""}"#.into())),
    r#"5 a 2 frame MSG {"data":"hi \"x\""}"#
  );
  assert_eq!(line(Body::Frame("A\nB".into())), "5 a 2 frame64 QQpC");
  assert_eq!(line(Body::Binary(vec![0, 255])), "5 a 2 binary AP8=");
  assert_eq!(line(Body::Binary(vec![])), "5 a 2 binary");
  assert_eq!(line(Body::Open("wss://x".into())), "5 a 2 open wss://x");
  assert_eq!(
    line(Body::Close("idle timeout".into())),
    "5 a 2 close idle timeout"
  );
  assert_eq!(line(Body::Close("".into())), "5 a 2 close");
  assert_eq!(line(Body::Close("a\r".into())), "5 a 2 close a\r");
  assert_eq!(line(Body::Frame("\r\n".into())), "5 a 2 frame64 DQo=");
}

#[test]
fn decode_errors() {
  let decode = |s: &str| Record::decode(s.as_bytes());
  assert_eq!(
    decode("5 a 2 later x"),
    Err(DecodeError::Unknown("later".into()))
  );
  for bad in [
    "",
    "5 a 2",
    "+5 a 2 frame x",
    "5 a -2 frame x",
    "5 a/b 2 frame x",
    "5 a 2 Frame x",
    "5 a 2 binary !!",
    "5  a 2 frame x",
  ] {
    assert!(matches!(decode(bad), Err(DecodeError::Bad(_))), "{bad:?}");
  }
}

#[test]
fn fetched_lines() {
  let line = |body| {
    let record = Record {
      receive_time: 5,
      instance: "b".into(),
      connection: 1,
      body,
    };
    let mut out = Vec::new();
    record.encode(&mut out);
    let encoded = String::from_utf8(out).unwrap();
    let encoded = encoded.strip_suffix('\n').unwrap().to_owned();
    assert_eq!(Record::decode(encoded.as_bytes()), Ok(record));
    encoded
  };
  let fetched = |text: &str| Body::Fetched {
    source: "rustlesearch".into(),
    timestamp: 1_794_146_765_482,
    nick: "feustang".into(),
    text: text.into(),
  };
  assert_eq!(
    line(fetched("hi  there")),
    "5 b 1 fetched rustlesearch 1794146765482 feustang hi  there"
  );
  assert_eq!(
    line(fetched("")),
    "5 b 1 fetched rustlesearch 1794146765482 feustang "
  );
  assert!(line(fetched("a\nb")).starts_with("5 b 1 fetched64 "));
  let range = Body::FetchRange {
    source: "rustlesearch".into(),
    from: 10,
    to: 20,
  };
  assert_eq!(line(range), "5 b 1 fetchrange rustlesearch 10 20");

  let decode = |s: &str| Record::decode(s.as_bytes());
  for bad in [
    "5 b 1 fetched rustlesearch 17x feustang hi",
    "5 b 1 fetched rustlesearch 17",
    "5 b 1 fetchrange rustlesearch 10",
    "5 b 1 fetchrange rustlesearch 10 20 30",
    "5 b 1 fetchrange rustlesearch -10 20",
  ] {
    assert!(matches!(decode(bad), Err(DecodeError::Bad(_))), "{bad:?}");
  }
}

#[test]
#[should_panic(expected = "not a valid token")]
fn fetched_nick_with_space_panics() {
  let record = Record {
    receive_time: 5,
    instance: "b".into(),
    connection: 1,
    body: Body::Fetched {
      source: "rustlesearch".into(),
      timestamp: 1,
      nick: "two words".into(),
      text: "x".into(),
    },
  };
  record.encode(&mut Vec::new());
}
