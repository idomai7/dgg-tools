//! Writer and Reader against real files.

use {
  ::journal::{
    DecodeError, Entry, ReadError, Reader, Record, Version, Writer,
    layout::{live_dir, live_files},
    record::v1::{self, Body},
  },
  ::std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
  },
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("journal-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn rec(receive_time: i64, frame: &str) -> v1::Record {
  v1::Record {
    receive_time,
    instance: "a".into(),
    connection: 1,
    body: Body::Frame(frame.into()),
  }
}

fn write(w: &mut Writer, r: &v1::Record) {
  w.write(r.receive_time, r.connection, r.body.clone())
    .unwrap();
}

fn record(e: Result<Entry, ReadError>) -> v1::Record {
  let Record::V1(r) = e.unwrap().record;
  r
}

fn frame(e: Result<Entry, ReadError>) -> String {
  match record(e).body {
    Body::Frame(f) => f,
    b => panic!("not a frame: {b:?}"),
  }
}

#[test]
fn write_read_and_rollover() {
  let root = TempRoot::new();
  let mut w = Writer::new(&root.0, "a");
  let recs = [
    rec(1_709_251_199_998, "MSG {\"n\":1}"),
    rec(1_709_251_199_999, "MSG {\"n\":2}"),
    rec(1_709_251_200_000, "MSG {\"n\":3}"),
  ];
  for r in &recs {
    write(&mut w, r);
  }
  drop(w);

  let files = live_files(&root.0).unwrap();
  let names: Vec<_> = files
    .iter()
    .map(|p| p.strip_prefix(live_dir(&root.0)).unwrap().to_owned())
    .collect();
  assert_eq!(
    names,
    [
      Path::new("2024-02-29").join("a.journal"),
      Path::new("2024-03-01").join("a.journal"),
    ]
  );
  let read: Vec<_> = files
    .iter()
    .flat_map(|f| Reader::open(f, 0).unwrap())
    .map(record)
    .collect();
  assert_eq!(read, recs);
}

#[test]
fn header_is_checked() {
  let root = TempRoot::new();
  let mut w = Writer::new(&root.0, "a");
  write(&mut w, &rec(0, "MSG 1"));
  drop(w);
  let path = live_files(&root.0).unwrap()[0].clone();
  let text = fs::read_to_string(&path).unwrap();
  assert_eq!(text, "journal 1.0\n0 a 1 frame MSG 1\n");

  let with = |header: &str| {
    fs::write(&path, text.replacen("journal 1.0", header, 1)).unwrap();
    Reader::open(&path, 0)
  };
  // A newer minor is read; kinds it added would come back as Unknown.
  let mut r = with("journal 1.7").unwrap();
  assert_eq!(r.version(), Some(Version { major: 1, minor: 7 }));
  assert_eq!(frame(r.next().unwrap()), "MSG 1");
  // A major without a decoder is refused.
  let v2 = Version { major: 2, minor: 0 };
  assert!(matches!(with("journal 2.0"), Err(ReadError::Version(v)) if v == v2));
  assert!(matches!(
    with("dgg-journal 1.0"),
    Err(ReadError::Header { found }) if found == "dgg-journal 1.0"
  ));
  // A header still being written reads as empty.
  fs::write(&path, "journ").unwrap();
  let mut r = Reader::open(&path, 0).unwrap();
  assert_eq!(r.version(), None);
  assert!(r.next().is_none());
}

#[test]
fn partial_tail_is_left_for_later() {
  let root = TempRoot::new();
  let mut w = Writer::new(&root.0, "a");
  write(&mut w, &rec(0, "MSG 1"));
  write(&mut w, &rec(0, "MSG 2"));
  drop(w);
  let path = &live_files(&root.0).unwrap()[0];
  let full = fs::read(path).unwrap();
  let mut third = Vec::new();
  rec(0, "MSG 3").encode(&mut third);
  let (head, tail) = third.split_at(10);
  let append = |bytes: &[u8]| {
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(bytes).unwrap();
  };
  append(head);

  let mut r = Reader::open(path, 0).unwrap();
  assert_eq!(r.by_ref().count(), 2);
  assert_eq!(r.offset(), full.len() as u64);

  append(tail);
  let rest: Vec<_> = Reader::open(path, r.offset()).unwrap().collect();
  assert_eq!(rest.len(), 1);
  assert_eq!(frame(rest.into_iter().next().unwrap()), "MSG 3");
}

#[test]
fn torn_line_is_isolated_on_reopen() {
  let root = TempRoot::new();
  let mut w = Writer::new(&root.0, "a");
  write(&mut w, &rec(0, "MSG 1"));
  drop(w);
  let path = live_files(&root.0).unwrap()[0].clone();
  let mut f = OpenOptions::new().append(true).open(&path).unwrap();
  f.write_all(b"0 a 1 fra").unwrap();
  drop(f);

  let mut w = Writer::new(&root.0, "a");
  write(&mut w, &rec(0, "MSG 2"));
  drop(w);
  let mut read = Reader::open(&path, 0).unwrap();
  assert_eq!(frame(read.next().unwrap()), "MSG 1");
  assert!(matches!(
    read.next(),
    Some(Err(ReadError::Line {
      error: DecodeError::Unknown(_),
      ..
    }))
  ));
  assert_eq!(frame(read.next().unwrap()), "MSG 2");
  assert!(read.next().is_none());
}
