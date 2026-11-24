//! Writer and Reader against real files.

use {
  ::common::temp_dir::TempDir,
  ::logger_journal::{
    DecodeError, Entry, Header, ReadError, Reader, Record, Variant, Version,
    Writer,
    layout::{backfill_files, live_dir, live_file, live_files, live_lock_file},
    record::v0::{self, Body},
    writer::{capturing, writing},
  },
  ::std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
  },
};

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
  let Record::V0(record) = entry.unwrap().record else {
    panic!("not dgg's")
  };
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
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::new(root.path(), "a");
  let records = [
    frame_record(1_709_251_199_998, "MSG {\"n\":1}"),
    frame_record(1_709_251_199_999, "MSG {\"n\":2}"),
    frame_record(1_709_251_200_000, "MSG {\"n\":3}"),
  ];
  for record in &records {
    write(&mut writer, record);
  }
  drop(writer);

  let files = live_files(root.path()).unwrap();
  let names: Vec<_> = files
    .iter()
    .map(|p| p.strip_prefix(live_dir(root.path())).unwrap().to_owned())
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
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::backfill(root.path(), "backfill-x").unwrap();
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

  assert!(live_files(root.path()).unwrap().is_empty());
  let files = backfill_files(root.path()).unwrap();
  let names: Vec<_> = files
    .iter()
    .map(|p| p.strip_prefix(root.path()).unwrap().to_owned())
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
fn a_capture_holds_its_lock_until_dropped() {
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::capture(root.path(), "a").unwrap();
  assert!(capturing(root.path(), "a").unwrap());
  // Not a backfill run's, nor another instance's.
  assert!(!writing(root.path(), "a").unwrap());
  assert!(!capturing(root.path(), "b").unwrap());
  // Only one writer at a time.
  assert!(Writer::capture(root.path(), "a").is_err());

  write(&mut writer, &frame_record(1_700_000_000_000, "{}"));
  drop(writer);
  assert!(!capturing(root.path(), "a").unwrap());
  assert!(!live_lock_file(root.path(), "a").exists());
  assert_eq!(live_files(root.path()).unwrap().len(), 1);
}

#[test]
fn header_is_checked() {
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  drop(writer);
  let path = live_files(root.path()).unwrap()[0].clone();
  let text = fs::read_to_string(&path).unwrap();
  assert_eq!(text, "journal dgg.0.3\u{1e}\n0 a 1 frame MSG 1\u{1e}\n");

  let with = |header: &str| {
    let text = text.replacen("journal dgg.0.3\u{1e}", header, 1);
    fs::write(&path, text).unwrap();
    Reader::open(&path, 0)
  };
  let mut reader = with("journal dgg.0.3\u{1e}").unwrap();
  assert_eq!(reader.version(), Some(Version { major: 0, minor: 3 }));
  assert_eq!(frame(reader.next().unwrap()), "MSG 1");
  // Older minors are no longer read.
  let v02 = Version { major: 0, minor: 2 };
  assert!(matches!(
    with("journal dgg.0.2\u{1e}"),
    Err(ReadError::Version(Variant::Dgg, v)) if v == v02
  ));
  // Within major 0 another minor may have broken anything: refused.
  let v07 = Version { major: 0, minor: 7 };
  assert!(matches!(
    with("journal dgg.0.7\u{1e}"),
    Err(ReadError::Version(Variant::Dgg, v)) if v == v07
  ));
  // As is a major without a decoder.
  let v2 = Version { major: 2, minor: 0 };
  assert!(matches!(
    with("journal dgg.2.0\u{1e}"),
    Err(ReadError::Version(Variant::Dgg, v)) if v == v2
  ));
  // Every header names its variant and ends at RS LF; older versions'
  // headers, which didn't, aren't read as headers at all.
  for header in [
    "journal dgg.0.3",
    "journal dgg.0.2",
    "journal 0.1",
    "journal 0.1\u{1e}",
  ] {
    assert!(matches!(with(header), Err(ReadError::Header { .. })));
  }
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
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  write(&mut writer, &frame_record(0, "MSG 2"));
  drop(writer);
  let path = &live_files(root.path()).unwrap()[0];
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
fn records_may_span_lines() {
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 1\nand 2\n"));
  write(&mut writer, &frame_record(0, "MSG 3"));
  drop(writer);
  let path = &live_files(root.path()).unwrap()[0];
  assert_eq!(
    fs::read_to_string(path).unwrap(),
    "journal dgg.0.3\u{1e}\n0 a 1 frame MSG 1\nand 2\n\u{1e}\n0 a 1 frame MSG 3\u{1e}\n"
  );
  let mut reader = Reader::open(path, 0).unwrap();
  assert_eq!(frame(reader.next().unwrap()), "MSG 1\nand 2\n");
  assert_eq!(frame(reader.next().unwrap()), "MSG 3");

  // A tail ending at a bare LF is still being written.
  let full = fs::read(path).unwrap().len() as u64;
  let mut file = OpenOptions::new().append(true).open(path).unwrap();
  file.write_all(b"0 a 1 frame MSG 4\n").unwrap();
  let mut reader = Reader::open(path, 0).unwrap();
  assert_eq!(reader.by_ref().count(), 2);
  assert_eq!(reader.offset(), full);
  file.write_all(b"more\x1e\n").unwrap();
  let mut rest = Reader::open(path, full).unwrap();
  assert_eq!(frame(rest.next().unwrap()), "MSG 4\nmore");
}

#[test]
fn records_are_counted_without_decoding() {
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::new(root.path(), "a");
  for n in 0..3 {
    write(&mut writer, &frame_record(0, &format!("MSG {n}")));
  }
  drop(writer);
  let path = &live_files(root.path()).unwrap()[0];
  let mut file = OpenOptions::new().append(true).open(path).unwrap();
  // Not a record this build could decode, but a record all the same; and a
  // partial one, which isn't counted until it's complete.
  file.write_all(b"nonsense\x1e\n0 a 1 frame MSG").unwrap();

  assert_eq!(Reader::open(path, 0).unwrap().count_records(10).unwrap(), 4);
  assert_eq!(Reader::open(path, 0).unwrap().count_records(2).unwrap(), 2);
  let mut reader = Reader::open(path, 0).unwrap();
  reader.next();
  let second = reader.offset();
  let rest = Reader::open(path, second).unwrap();
  assert_eq!(rest.count_records(10).unwrap(), 3);
}

#[test]
fn older_files_are_never_appended_to() {
  let root = TempDir::new("logger-journal-files");
  let day = live_file(root.path(), "1970-01-01", "a");
  fs::create_dir_all(day.parent().unwrap()).unwrap();
  // A file of 0.1: a record per line, an LF in data as base64.
  fs::write(&day, "journal 0.1\n0 a 1 frame64 QQpC\n0 a 1 frame B\n").unwrap();
  assert!(matches!(
    Reader::open(&day, 0),
    Err(ReadError::Header { .. })
  ));

  // The writer starts the day's next file, in the new format.
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  drop(writer);
  let files = live_files(root.path()).unwrap();
  let names: Vec<_> = files
    .iter()
    .map(|f| f.file_name().unwrap().to_owned())
    .collect();
  assert_eq!(names, ["a.2.journal", "a.journal"]);
  assert_eq!(
    fs::read_to_string(&files[0]).unwrap(),
    "journal dgg.0.3\u{1e}\n0 a 1 frame MSG 1\u{1e}\n"
  );
  assert_eq!(
    fs::read_to_string(&day).unwrap(),
    "journal 0.1\n0 a 1 frame64 QQpC\n0 a 1 frame B\n"
  );
  // And appends to that one after.
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 2"));
  drop(writer);
  assert_eq!(live_files(root.path()).unwrap().len(), 2);
  let header = Header::new(Variant::Dgg, v0::VERSION);
  assert_eq!(header.text(), "journal dgg.0.3\u{1e}\n");
}

#[test]
fn torn_line_is_isolated_on_reopen() {
  let root = TempDir::new("logger-journal-files");
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 1"));
  drop(writer);
  let path = live_files(root.path()).unwrap()[0].clone();
  let mut file = OpenOptions::new().append(true).open(&path).unwrap();
  file.write_all(b"0 a 1 fra").unwrap();
  drop(file);

  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 2"));
  drop(writer);
  let mut read = Reader::open(&path, 0).unwrap();
  assert_eq!(frame(read.next().unwrap()), "MSG 1");
  assert!(matches!(
    read.next(),
    Some(Err(ReadError::Record {
      error: DecodeError::Unknown(_),
      ..
    }))
  ));
  assert_eq!(frame(read.next().unwrap()), "MSG 2");
  assert!(read.next().is_none());

  // Torn after an LF in its data, too.
  let mut file = OpenOptions::new().append(true).open(&path).unwrap();
  file.write_all(b"0 a 1 frame MSG\n").unwrap();
  drop(file);
  let mut writer = Writer::new(root.path(), "a");
  write(&mut writer, &frame_record(0, "MSG 3"));
  drop(writer);
  let last: Vec<_> = Reader::open(&path, 0).unwrap().skip(3).collect();
  assert_eq!(last.len(), 2);
  assert_eq!(frame(last.into_iter().nth(1).unwrap()), "MSG 3");
}
