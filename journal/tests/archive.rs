use {
  ::journal::{
    Reader, Record,
    archive::{self, Archive},
    layout::{archive_file, archive_month_dir},
    record::v1::Body,
  },
  ::std::{
    fs,
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
      .join(format!("journal-archive-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn entry(day: &str, lines: u64) -> Archive {
  Archive {
    day: day.into(),
    compressed: 10,
    bytes: 20,
    lines,
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
  let text = archive::encode(&[entry("2026-11-09", 2), entry("2026-11-08", 1)]);
  let sha = "ab".repeat(32);
  assert_eq!(
    text,
    format!(
      "journal-archive 1.0\n2026-11-08 10 20 1 {sha}\n\
       2026-11-09 10 20 2 {sha}\n"
    )
  );
  let decoded = archive::decode(&text).unwrap();
  assert_eq!(decoded, [entry("2026-11-08", 1), entry("2026-11-09", 2)]);
  // A newer minor, with an entry this version doesn't know.
  let newer =
    format!("journal-archive 1.2\nsomething new\n2026-11-08 10 20 1 {sha}\n");
  assert_eq!(archive::decode(&newer).unwrap(), [entry("2026-11-08", 1)]);
  for bad in [
    "journal-archive 2.0\n".to_owned(),
    format!("journal-archive 1.0\n2026-11-08 10 x 1 {sha}\n"),
    "journal-archive 1.0\n2026-11-08 10 20 1 ABCD\n".to_owned(),
  ] {
    assert_eq!(archive::decode(&bad), None, "{bad:?}");
  }
}

#[test]
fn recording_replaces_and_lists() {
  let root = TempRoot::new();
  assert_eq!(archive::all(&root.0).unwrap(), []);
  archive::record(&root.0, &entry("2026-12-01", 1)).unwrap();
  archive::record(&root.0, &entry("2026-11-08", 1)).unwrap();
  archive::record(&root.0, &entry("2026-11-08", 5)).unwrap();
  archive::record(&root.0, &entry("2025-01-31", 1)).unwrap();
  let days: Vec<_> = archive::all(&root.0)
    .unwrap()
    .into_iter()
    .map(|archive| (archive.day, archive.lines))
    .collect();
  assert_eq!(
    days,
    [
      ("2025-01-31".into(), 1),
      ("2026-11-08".into(), 5),
      ("2026-12-01".into(), 1),
    ]
  );
  assert_eq!(archive::read_month(&root.0, "2026-11-30").unwrap().len(), 1);
}

#[test]
fn reader_reads_archives_and_resumes() {
  let root = TempRoot::new();
  let path = archive_file(&root.0, "1970-01-01");
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  let journal = "journal 1.1\n0 a 1 frame MSG 1\n5 b 1 frame MSG 2\n";
  let compressed = ::zstd::encode_all(journal.as_bytes(), 3).unwrap();
  fs::write(&path, compressed).unwrap();

  let frames = |offset| -> Vec<(u64, String)> {
    Reader::open(&path, offset)
      .unwrap()
      .map(|entry| {
        let entry = entry.unwrap();
        let Record::V1(record) = entry.record;
        let Body::Frame(frame) = record.body else {
          panic!("not a frame")
        };
        (entry.offset, frame)
      })
      .collect()
  };
  assert_eq!(frames(0), [(12, "MSG 1".into()), (30, "MSG 2".into())]);
  // Offsets are into the decompressed journal, and resuming works.
  assert_eq!(frames(30), [(30, "MSG 2".into())]);
}
