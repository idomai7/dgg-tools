//! Kick's variant: its header, records and writer. Endpoints and slugs are made
//! up.

use {
  ::common::temp_dir::TempDir,
  ::logger_journal::{
    DecodeError, Header, Reader, Record, Variant, Version,
    layout::{channel_root, live_files},
    record::kick::v0::{self, Body},
    writer::kick::Writer,
  },
  ::std::{fs, path::Path},
};

const DAY: i64 = 1_700_000_000_000;
const A: &str = "wss://a.example.com/connection/websocket";
const B: &str = "wss://b.example.com/connection/websocket";

#[test]
fn header() {
  let header = Header::new(Variant::Kick, v0::VERSION)
    .with("endpoint", A)
    .with("channel", "one");
  let line = format!("journal kick.0.1 ; endpoint={A} channel=one");
  assert_eq!(header.line(), line);
  // It ends at RS LF, as its records do.
  assert_eq!(header.text(), format!("{line}\u{1e}\n"));
  let parsed = Header::parse(format!("{line}\u{1e}").as_bytes());
  assert_eq!(parsed, Some(header.clone()));
  assert_eq!(Header::parse(line.as_bytes()), None);
  assert_eq!(header.get("channel"), Some("one"));
  assert_eq!(header.get("other"), None);
  assert!(header.readable());

  // Keys a reader doesn't know are kept for it to skip.
  let dgg = Header::new(Variant::Dgg, Version { major: 0, minor: 3 });
  assert_eq!(dgg.line(), "journal dgg.0.3");
  assert_eq!(Header::parse(b"journal dgg.0.3\x1e"), Some(dgg.clone()));
  let noted = Header::parse(b"journal dgg.0.3 ; new_key=1\x1e").unwrap();
  assert_eq!(noted, dgg.with("new_key", "1"));

  let kick =
    |major, minor| Header::new(Variant::Kick, Version { major, minor });
  // 0.0's files are read too: 0.1 only added a kind.
  assert!(kick(0, 0).readable());
  assert!(kick(0, 1).readable());
  assert!(!kick(0, 2).readable());
  assert!(!kick(1, 0).readable());
  for bad in [
    "journal kick.0",
    "journal twitch.0.0",
    "journal kick.0.0 ;",
    "journal kick.0.0 ; ",
    "journal kick.0.0 ; channel",
    "journal kick.0.0 ; Channel=one",
    "journal kick.0.0 ; channel=one  endpoint=x",
    "journal kick.0.0 channel=one",
    // Before variants, dgg's named none; no longer read.
    "journal 0.1",
  ] {
    let bad = format!("{bad}\u{1e}");
    assert_eq!(Header::parse(bad.as_bytes()), None, "{bad:?}");
  }
}

#[test]
fn lines_round_trip() {
  let frame = "{\"connect\":{}}\n{\"push\":{}}";
  let rs = "{}\u{1e}";
  for (body, encoded) in [
    (
      Body::Frame("{}".into()),
      "5 i 2 frame {}\u{1e}\n".to_owned(),
    ),
    (
      Body::Frame(frame.into()),
      format!("5 i 2 frame {frame}\u{1e}\n"),
    ),
    (
      Body::Frame(rs.into()),
      format!("5 i 2 frame64 {}\u{1e}\n", base64(rs)),
    ),
    (Body::Binary(vec![0, 1]), "5 i 2 binary AAE=\u{1e}\n".into()),
    (Body::Open(A.into()), format!("5 i 2 open {A}\u{1e}\n")),
    (Body::Close(String::new()), "5 i 2 close\u{1e}\n".into()),
    (
      Body::History {
        cursor: None,
        page: frame.into(),
      },
      format!("5 i 2 history - {frame}\u{1e}\n"),
    ),
    (
      Body::History {
        cursor: Some("1795462741539000".into()),
        page: "{}".into(),
      },
      "5 i 2 history 1795462741539000 {}\u{1e}\n".into(),
    ),
    (
      Body::History {
        cursor: None,
        page: rs.into(),
      },
      format!("5 i 2 history64 {}\u{1e}\n", base64(&format!("- {rs}"))),
    ),
  ] {
    let record = v0::Record {
      receive_time: 5,
      instance: "i".into(),
      connection: 2,
      body,
    };
    let mut out = Vec::new();
    record.encode(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), encoded);
    let line = encoded.strip_suffix("\u{1e}\n").unwrap().as_bytes();
    assert_eq!(v0::Record::decode(line), Ok(record));
  }
  // dgg's kinds aren't Kick's.
  assert_eq!(
    v0::Record::decode(b"5 i 2 fetchrange s 1 2"),
    Err(DecodeError::Unknown("fetchrange".into()))
  );
  assert!(matches!(
    v0::Record::decode(b"5 i 2 frame {}\x1e{}"),
    Err(DecodeError::Bad(_))
  ));
  for bad in [&b"5 i 2 history -"[..], b"5 i 2 history"] {
    assert!(matches!(v0::Record::decode(bad), Err(DecodeError::Bad(_))));
  }
}

fn base64(text: &str) -> String {
  use ::base64::{Engine, engine::general_purpose::STANDARD};
  STANDARD.encode(text)
}

/// Each file's header, and its records' bodies.
fn read(root: &Path) -> Vec<(String, Vec<Body>)> {
  live_files(root)
    .unwrap()
    .iter()
    .map(|file| {
      let reader = Reader::open(file, 0).unwrap();
      let name = file.file_name().unwrap().to_string_lossy().into_owned();
      let header = reader.header().unwrap().line();
      let bodies = reader
        .map(|entry| {
          let Record::Kick0(record) = entry.unwrap().record else {
            panic!("not Kick's")
          };
          record.body
        })
        .collect();
      (format!("{name}: {header}"), bodies)
    })
    .collect()
}

#[test]
fn a_new_endpoint_starts_the_next_file() {
  let dir = TempDir::new("logger-journal-kick");
  let root = channel_root(dir.path(), "one");
  let mut writer = Writer::new(&root, "i", "one");
  let frame = || Body::Frame("{}".into());
  for (connection, body) in [
    (1, Body::Open(A.into())),
    (1, frame()),
    (1, Body::Close("3005 gone".into())),
    (2, Body::Open(A.into())),
    (2, Body::Close("again".into())),
    (3, Body::Open(B.into())),
    (3, frame()),
    // Back to the first: still a new file, so files go in order.
    (4, Body::Open(A.into())),
  ] {
    writer.write(DAY, connection, body).unwrap();
  }
  drop(writer);

  let header =
    |endpoint| format!("journal kick.0.1 ; endpoint={endpoint} channel=one");
  assert_eq!(
    read(&root),
    [
      (
        format!("i.2.journal: {}", header(B)),
        vec![Body::Open(B.into()), frame()],
      ),
      (
        format!("i.3.journal: {}", header(A)),
        vec![Body::Open(A.into())],
      ),
      (
        format!("i.journal: {}", header(A)),
        vec![
          Body::Open(A.into()),
          frame(),
          Body::Close("3005 gone".into()),
          Body::Open(A.into()),
          Body::Close("again".into()),
        ],
      ),
    ]
  );
}

#[test]
fn rolls_over_at_midnight_and_carries_on_after_a_restart() {
  let dir = TempDir::new("logger-journal-kick");
  let root = channel_root(dir.path(), "one");
  let day = 24 * 60 * 60 * 1000;
  let mut writer = Writer::new(&root, "i", "one");
  writer.write(DAY, 1, Body::Open(A.into())).unwrap();
  writer
    .write(DAY + day, 1, Body::Frame("{}".into()))
    .unwrap();
  drop(writer);
  // The same endpoint again, as after a restart: appended to.
  let mut writer = Writer::new(&root, "i", "one");
  writer.write(DAY + day, 1, Body::Open(A.into())).unwrap();
  drop(writer);

  let files = live_files(&root).unwrap();
  assert_eq!(files.len(), 2);
  let texts: Vec<String> = files
    .iter()
    .map(|f| fs::read_to_string(f).unwrap())
    .collect();
  let header = format!("journal kick.0.1 ; endpoint={A} channel=one\u{1e}\n");
  assert_eq!(texts[0], format!("{header}{DAY} i 1 open {A}\u{1e}\n"));
  let next = DAY + day;
  assert_eq!(
    texts[1],
    format!("{header}{next} i 1 frame {{}}\u{1e}\n{next} i 1 open {A}\u{1e}\n")
  );
}

#[test]
fn lines_before_any_open_go_in_a_file_with_no_endpoint() {
  let dir = TempDir::new("logger-journal-kick");
  let mut writer = Writer::new(dir.path(), "i", "one");
  writer.write(DAY, 1, Body::Close("why".into())).unwrap();
  assert!(writer.write(DAY, 2, Body::Open("a b".into())).is_err());
  drop(writer);
  let files = live_files(dir.path()).unwrap();
  let text = fs::read_to_string(&files[0]).unwrap();
  assert_eq!(
    text,
    format!(
      "journal kick.0.1 ; channel=one\u{1e}\n{DAY} i 1 close why\u{1e}\n"
    )
  );
}

#[test]
fn history_cursors_must_be_tokens() {
  let dir = TempDir::new("logger-journal-kick");
  let mut writer = Writer::new(dir.path(), "i", "one");
  for cursor in ["-", "a b", ""] {
    let body = Body::History {
      cursor: Some(cursor.into()),
      page: "{}".into(),
    };
    assert!(writer.write(DAY, 1, body).is_err(), "{cursor:?}");
  }
}

#[test]
fn reads_0_0_and_starts_the_next_file() {
  let dir = TempDir::new("logger-journal-kick");
  let root = channel_root(dir.path(), "one");
  // A file kick-mod wrote at 0.0.
  let old = root.join("live/2023-11-14/i.journal");
  fs::create_dir_all(old.parent().unwrap()).unwrap();
  let header = format!("journal kick.0.0 ; endpoint={A} channel=one\u{1e}\n");
  fs::write(&old, format!("{header}{DAY} i 1 open {A}\u{1e}\n")).unwrap();
  assert_eq!(
    read(&root),
    [(
      format!("i.journal: journal kick.0.0 ; endpoint={A} channel=one"),
      vec![Body::Open(A.into())],
    )]
  );

  // Written at 0.1, to the day's next file.
  let mut writer = Writer::new(&root, "i", "one");
  writer.write(DAY, 1, Body::Open(A.into())).unwrap();
  drop(writer);
  let names: Vec<_> = read(&root).into_iter().map(|(name, _)| name).collect();
  assert_eq!(
    names,
    [
      format!("i.2.journal: journal kick.0.1 ; endpoint={A} channel=one"),
      format!("i.journal: journal kick.0.0 ; endpoint={A} channel=one"),
    ]
  );
}
