use {
  ::common::temp_dir::TempDir,
  ::logger_journal::{
    Reader, Record, Variant,
    archive::{self, Archive},
    layout::{archive_file, archive_month_dir},
    record::v0::Body,
  },
  ::std::{fs, path::Path},
};

fn entry(day: &str, records: u64) -> Archive {
  Archive {
    day: day.into(),
    compressed: 10,
    bytes: 20,
    records,
    sha256: "ab".repeat(32),
  }
}

#[test]
fn paths() {
  let root = Path::new("j");
  assert_eq!(
    archive_file(root, "2026-11-08"),
    Path::new("j/archive/2026/11/2026-11-08.journal.zst")
  );
  assert_eq!(
    archive_month_dir(root, "2026-11-08"),
    Path::new("j/archive/2026/11")
  );
}

#[test]
fn manifest_format() {
  let entries = [entry("2026-11-09", 2), entry("2026-11-08", 1)];
  let text = archive::encode(Variant::Dgg, &entries);
  let sha = "ab".repeat(32);
  // Entries end at RS LF.
  let rows =
    format!("2026-11-08 10 20 1 {sha}\u{1e}\n2026-11-09 10 20 2 {sha}\u{1e}\n");
  assert_eq!(text, format!("journal-archive dgg.0.2\u{1e}\n{rows}"));
  let sorted = vec![entry("2026-11-08", 1), entry("2026-11-09", 2)];
  let decoded = archive::decode(&text).unwrap();
  assert_eq!(decoded, (Variant::Dgg, sorted.clone()));
  let kick = archive::encode(Variant::Kick, &entries);
  assert_eq!(kick, format!("journal-archive kick.0.0\u{1e}\n{rows}"));
  assert_eq!(archive::decode(&kick).unwrap(), (Variant::Kick, sorted));
  // An entry this version doesn't know is skipped, as are informational
  // fields.
  let extra = format!(
    "journal-archive dgg.0.2 ; note=x\u{1e}\nsomething new\u{1e}\n\
     2026-11-08 10 20 1 {sha}\u{1e}\n"
  );
  let first = vec![entry("2026-11-08", 1)];
  assert_eq!(archive::decode(&extra).unwrap(), (Variant::Dgg, first));
  for bad in [
    // Within major 0, only the exact versions are read: older ones no
    // longer, every host's manifests having been migrated.
    format!("journal-archive 0.0\n2026-11-08 10 20 1 {sha}\n"),
    format!("journal-archive dgg.0.1\n2026-11-08 10 20 1 {sha}\u{1e}\n"),
    "journal-archive dgg.0.2\n".to_owned(),
    "journal-archive kick.0.1\n".to_owned(),
    "journal-archive dgg.1.0\n".to_owned(),
    // Each version's header has one form.
    "journal-archive 0.1\n".to_owned(),
    "journal-archive dgg.0.0\n".to_owned(),
    format!("journal-archive dgg.0.2\u{1e}\n2026-11-08 10 x 1 {sha}\u{1e}\n"),
    "journal-archive dgg.0.2\u{1e}\n2026-11-08 10 20 1 ABCD\u{1e}\n".to_owned(),
    // An entry without its end.
    format!("journal-archive dgg.0.2\u{1e}\n2026-11-08 10 20 1 {sha}\n"),
  ] {
    assert_eq!(archive::decode(&bad), None, "{bad:?}");
  }
}

#[test]
fn recording_replaces_and_lists() {
  let root = TempDir::new("logger-journal-archive");
  assert_eq!(archive::all(root.path()).unwrap(), []);
  let record = |archive| archive::record(root.path(), Variant::Dgg, &archive);
  record(entry("2026-12-01", 1)).unwrap();
  record(entry("2026-11-08", 1)).unwrap();
  record(entry("2026-11-08", 5)).unwrap();
  record(entry("2025-01-31", 1)).unwrap();
  // Not into another variant's.
  let kick =
    archive::record(root.path(), Variant::Kick, &entry("2026-11-09", 1));
  assert!(kick.is_err());
  let days: Vec<_> = archive::all(root.path())
    .unwrap()
    .into_iter()
    .map(|archive| (archive.day, archive.records))
    .collect();
  assert_eq!(
    days,
    [
      ("2025-01-31".into(), 1),
      ("2026-11-08".into(), 5),
      ("2026-12-01".into(), 1),
    ]
  );
  assert_eq!(
    archive::read_month(root.path(), "2026-11-30")
      .unwrap()
      .len(),
    1
  );
}

#[test]
fn reader_reads_archives_and_resumes() {
  let root = TempDir::new("logger-journal-archive");
  let path = archive_file(root.path(), "1970-01-01");
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  let journal = "journal dgg.0.3\u{1e}\n0 a 1 frame MSG 1\u{1e}\n\
                 5 b 1 frame MSG 2\u{1e}\n";
  let compressed = ::zstd::encode_all(journal.as_bytes(), 3).unwrap();
  fs::write(&path, compressed).unwrap();

  let frames = |offset| -> Vec<(u64, String)> {
    Reader::open(&path, offset)
      .unwrap()
      .map(|entry| {
        let entry = entry.unwrap();
        let Record::V0(record) = entry.record else {
          panic!("not dgg's")
        };
        let Body::Frame(frame) = record.body else {
          panic!("not a frame")
        };
        (entry.offset, frame)
      })
      .collect()
  };
  assert_eq!(frames(0), [(17, "MSG 1".into()), (36, "MSG 2".into())]);
  // Offsets are into the decompressed journal, and resuming works.
  assert_eq!(frames(36), [(36, "MSG 2".into())]);
}
