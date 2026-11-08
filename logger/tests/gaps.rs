use {
  ::journal::{Writer, record::v1::Body},
  ::logger::{
    gaps::{Gap, coverage, format_duration, format_utc},
    index::{index, open},
  },
  ::std::{
    fs,
    path::PathBuf,
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
      .join(format!("logger-gaps-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn history(timestamps: &[i64]) -> Body {
  let entries: Vec<_> = timestamps
    .iter()
    .map(|&timestamp| {
      let message = ::serde_json::json!({
        "nick": "n",
        "timestamp": timestamp,
        "data": format!("at {timestamp}"),
      });
      ::serde_json::Value::String(format!("MSG {message}"))
    })
    .collect();
  Body::Frame(format!("HISTORY {}", ::serde_json::Value::Array(entries)))
}

#[test]
fn finds_gaps_and_coverage() {
  let root = TempRoot::new();
  let journal = root.0.join("journal");
  let open_url = || Body::Open("wss://x".into());

  // a#1: 1000..2000, closed. a#2: 2500..3000, never closed (crashed); its
  // HISTORY reaches back to 1500, before the 2000..2500 gap.
  let mut a = Writer::new(&journal, "a");
  a.write(1000, 1, open_url()).unwrap();
  a.write(2000, 1, Body::Close("reset".into())).unwrap();
  a.write(2500, 2, open_url()).unwrap();
  a.write(2501, 2, history(&[1500, 2200])).unwrap();
  a.write(3000, 2, Body::Frame("JOIN {}".into())).unwrap();
  drop(a);
  // c overlaps a#1 entirely, so adds no gap.
  let mut c = Writer::new(&journal, "c");
  c.write(1200, 1, open_url()).unwrap();
  c.write(1800, 1, Body::Close("shutdown".into())).unwrap();
  drop(c);
  // b resumes after the 3000..3500 gap, but its HISTORY only reaches 3200.
  let mut b = Writer::new(&journal, "b");
  b.write(3500, 1, open_url()).unwrap();
  b.write(3501, 1, history(&[3200, 3400])).unwrap();
  b.write(4000, 1, Body::Close("shutdown".into())).unwrap();
  drop(b);

  let mut db = open(&root.0.join("index.sqlite")).unwrap();
  index(&mut db, &journal).unwrap();
  let coverage = coverage(&db).unwrap().unwrap();
  assert_eq!((coverage.first, coverage.last), (1000, 4000));
  assert_eq!(
    coverage.gaps,
    [
      Gap {
        start: 2000,
        end: 2500,
        resumed_by: ("a".into(), 2),
        history_from: Some(1500),
        recovered: 1,
        backfilled: false,
      },
      Gap {
        start: 3000,
        end: 3500,
        resumed_by: ("b".into(), 1),
        history_from: Some(3200),
        recovered: 2,
        backfilled: false,
      },
    ]
  );
  assert!(coverage.gaps[0].covered());
  assert!(!coverage.gaps[1].covered());
}

#[test]
fn no_connections_no_coverage() {
  let root = TempRoot::new();
  let db = open(&root.0.join("index.sqlite")).unwrap();
  assert_eq!(coverage(&db).unwrap(), None);
}

#[test]
fn formats() {
  assert_eq!(format_utc(0), "1970-01-01 00:00:00.000");
  assert_eq!(format_utc(1_794_128_504_611), "2026-11-08 09:01:44.611");
  assert_eq!(format_duration(850), "850ms");
  assert_eq!(format_duration(12_345), "12.3s");
  assert_eq!(format_duration(245_000), "4m05s");
  assert_eq!(format_duration(7_620_000), "2h07m");
  assert_eq!(format_duration(273_600_000), "3d04h");
}

#[test]
fn fetched_spans_fill_gaps() {
  let root = TempRoot::new();
  let journal = root.0.join("journal");
  let mut a = Writer::new(&journal, "a");
  a.write(1000, 1, Body::Open("wss://x".into())).unwrap();
  a.write(2000, 1, Body::Close("reset".into())).unwrap();
  a.write(5000, 2, Body::Open("wss://x".into())).unwrap();
  a.write(6000, 2, Body::Close("reset".into())).unwrap();
  a.write(8000, 3, Body::Open("wss://x".into())).unwrap();
  drop(a);
  // Two touching spans cover 2000..5000; 6000..8000 is only half fetched.
  let mut backfill = Writer::new(&journal, "backfill-x");
  for (from, to) in [(1900, 3000), (3000, 5100), (7000, 8100)] {
    let range = Body::FetchRange {
      source: "rustlesearch".into(),
      from,
      to,
    };
    backfill.write(9000, 1, range).unwrap();
  }
  drop(backfill);

  let mut db = open(&root.0.join("index.sqlite")).unwrap();
  index(&mut db, &journal).unwrap();
  let gaps = coverage(&db).unwrap().unwrap().gaps;
  let summary: Vec<_> = gaps
    .iter()
    .map(|gap| (gap.start, gap.end, gap.backfilled, gap.filled()))
    .collect();
  assert_eq!(
    summary,
    [(2000, 5000, true, true), (6000, 8000, false, false)]
  );
}
