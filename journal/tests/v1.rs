use ::journal::{
  DecodeError,
  v1::{Body, Record},
};

#[test]
fn line_format() {
  let line = |body| {
    let r = Record {
      recv: 5,
      inst: "a".into(),
      conn: 2,
      body,
    };
    let mut out = Vec::new();
    r.encode(&mut out);
    let s = String::from_utf8(out).unwrap();
    let s = s.strip_suffix('\n').unwrap().to_owned();
    assert_eq!(Record::decode(s.as_bytes()), Ok(r));
    s
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
  assert_eq!(line(Body::Close("a\r".into())), "5 a 2 close64 YQ0=");
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
