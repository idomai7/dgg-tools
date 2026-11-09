//! Writer and Reader against real files.

use {
  ::journal::{
    DecodeError, Entry, ReadError, Reader, Record, Version, Writer,
    layout::{backfill_files, live_dir, live_files},
    record::v0::{self, Body},
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

fn frame_record(receive_time: i64, frame: &str) -> v0::Record {
  v0::Record {
    receive_time,
    instance: "a".into(),
    connection: 1,
    body: Body::Frame(frame.into()),
  }
}

fn write(writer: &mut Writer, record: &v0::Record) {
  writer
    .write(record.receive_time, record.connection, record.body.clone())
    .unwrap();
}

fn record(entry: Result<Entry, ReadError>) -> v0::Record {
  let Record::V0(record) = entry.unwrap().record;
  record
}

fn frame(entry: Result<Entry, ReadError>) -> String {
  match record(entry).body {
    Body::Frame(frame) => frame,
    body => panic!("not a frame: {body:?}"),
  }
}

#[test]
fn write_read_and_rollover() {
  let root = TempRoot::new();
  let mut writer = Writer::new(&root.0, "a");
  let records = [
    frame_record(1_709_251_199_998, "MSG {\"n\":1}"),
    frame_record(1_709_251_199_999, "MSG {\"n\":2}"),
    frame_record(1_709_251_200_000, "MSG {\"n\":3}"),
  ];
  for record in &records {
    write(&mut writer, record);
  }
  drop(writer);

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
    .flat_map(|file| Reader::open(file, 0).unwrap())
    .map(record)
    .collect();
  assert_eq!(read, records);
}

#[test]
fn backfill_files_by_the_given_day() {
  let root = TempRoot::new();
  let mut writer = Writer::backfill(&root.0, "backfill-x");
  let fetched = |timestamp| Body::Fetched {
    source: "linkers".into(),
    timestamp,
    nick: "n".into(),
    text: "t".into(),
  };
  // Fetched today, filed under the messages' days.
  let now = 1_794_168_240_000;
  for timestamp in [1_451_692_683_000, 1_451_606_400_000, 1_451_606_399_999] {
    writer
      .write_on(timestamp, now, 1, fetched(timestamp))
      .unwrap();
  }
  drop(writer);

  assert!(live_files(&root.0).unwrap().is_empty());
  let files = backfill_files(&root.0).unwrap();
  let names: Vec<_> = files
    .iter()
    .map(|p| p.strip_prefix(&root.0).unwrap().to_owned())
    .collect();
  assert_eq!(
    names,
    [
      Path::new("backfill/live/2015-12-31").join("backfill-x.journal"),
      Path::new("backfill/live/2016-01-01").join("backfill-x.journal"),
    ]
  );
  let read: Vec<_> = files
    .iter()
    .flat_map(|file| Reader::open(file, 0).unwrap())
    .map(record)
    .collect();
  assert_eq!(read.len(), 3);
  assert!(read.iter().all(|record| record.receive_time == now));
}

#[test]
fn header_is_checked() {
  let root = TempRoot::new();
  let mut writer = Writer::new(&root.0, "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  drop(writer);
  let path = live_files(&root.0).unwrap()[0].clone();
  let text = fs::read_to_string(&path).unwrap();
  assert_eq!(text, "journal 0.1\n0 a 1 frame MSG 1\n");

  let with = |header: &str| {
    fs::write(&path, text.replacen("journal 0.1", header, 1)).unwrap();
    Reader::open(&path, 0)
  };
  let mut reader = with("journal 0.1").unwrap();
  assert_eq!(reader.version(), Some(Version { major: 0, minor: 1 }));
  assert_eq!(frame(reader.next().unwrap()), "MSG 1");
  // Within major 0 another minor may have broken anything: refused.
  let v07 = Version { major: 0, minor: 7 };
  assert!(
    matches!(with("journal 0.7"), Err(ReadError::Version(v)) if v == v07)
  );
  // As is a major without a decoder.
  let v2 = Version { major: 2, minor: 0 };
  assert!(matches!(with("journal 2.0"), Err(ReadError::Version(v)) if v == v2));
  assert!(matches!(
    with("dgg-journal 1.0"),
    Err(ReadError::Header { found }) if found == "dgg-journal 1.0"
  ));
  // A header still being written reads as empty.
  fs::write(&path, "journ").unwrap();
  let mut reader = Reader::open(&path, 0).unwrap();
  assert_eq!(reader.version(), None);
  assert!(reader.next().is_none());
}

#[test]
fn partial_tail_is_left_for_later() {
  let root = TempRoot::new();
  let mut writer = Writer::new(&root.0, "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  write(&mut writer, &frame_record(0, "MSG 2"));
  drop(writer);
  let path = &live_files(&root.0).unwrap()[0];
  let full = fs::read(path).unwrap();
  let mut third = Vec::new();
  frame_record(0, "MSG 3").encode(&mut third);
  let (head, tail) = third.split_at(10);
  let append = |bytes: &[u8]| {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
  };
  append(head);

  let mut reader = Reader::open(path, 0).unwrap();
  assert_eq!(reader.by_ref().count(), 2);
  assert_eq!(reader.offset(), full.len() as u64);

  append(tail);
  let rest: Vec<_> = Reader::open(path, reader.offset()).unwrap().collect();
  assert_eq!(rest.len(), 1);
  assert_eq!(frame(rest.into_iter().next().unwrap()), "MSG 3");
}

#[test]
fn torn_line_is_isolated_on_reopen() {
  let root = TempRoot::new();
  let mut writer = Writer::new(&root.0, "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  drop(writer);
  let path = live_files(&root.0).unwrap()[0].clone();
  let mut file = OpenOptions::new().append(true).open(&path).unwrap();
  file.write_all(b"0 a 1 fra").unwrap();
  drop(file);

  let mut writer = Writer::new(&root.0, "a");
  write(&mut writer, &frame_record(0, "MSG 2"));
  drop(writer);
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
