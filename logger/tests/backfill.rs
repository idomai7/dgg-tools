use {
  ::journal::{
    Reader, Record,
    layout::live_files,
    record::v1::{self, Body},
  },
  ::logger::{
    backfill::{
      Message, Options, Page, Source, run,
      rustlesearch::{parse, parse_timestamp, remaining},
    },
    gaps::Gap,
  },
  ::std::{
    fs,
    path::PathBuf,
    sync::{
      Mutex,
      atomic::{AtomicU32, Ordering},
    },
    time::Duration,
  },
};

/// A fresh directory, removed when dropped.
struct TempRoot(PathBuf);

impl TempRoot {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("logger-backfill-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

/// A message every 100 ms from 0 to 10 s, served five per page, recording
/// the cursors it was asked for.
struct Fake {
  messages: Vec<Message>,
  remaining: Option<u64>,
  asked: Mutex<Vec<i64>>,
}

impl Fake {
  fn new(remaining: Option<u64>) -> Self {
    let messages = (0..100)
      .rev()
      .map(|i| Message {
        timestamp: i * 100,
        nick: if i == 42 {
          "two words".into()
        } else {
          "n".into()
        },
        text: format!("at {}", i * 100),
      })
      .collect();
    Self {
      messages,
      remaining,
      asked: Mutex::new(Vec::new()),
    }
  }
}

impl Source for Fake {
  fn name(&self) -> &str {
    "fake"
  }

  async fn page(&self, before: i64) -> Result<Page, String> {
    self.asked.lock().unwrap().push(before);
    let messages: Vec<_> = self
      .messages
      .iter()
      .filter(|message| message.timestamp < before)
      .take(5)
      .cloned()
      .collect();
    Ok(Page {
      exhausted: messages.is_empty(),
      messages,
      remaining: self.remaining,
    })
  }
}

fn gap(start: i64, end: i64) -> Gap {
  Gap {
    start,
    end,
    resumed_by: ("a".into(), 1),
    history_from: None,
    recovered: 0,
  }
}

fn options(root: &TempRoot, max_requests: u64) -> Options {
  Options {
    journal_root: root.0.join("journal"),
    instance: "backfill-t".into(),
    margin: Duration::from_millis(500),
    max_requests,
    reserve: 10,
    delay: Duration::ZERO,
  }
}

fn bodies(root: &TempRoot) -> Vec<v1::Body> {
  live_files(&root.0.join("journal"))
    .unwrap()
    .iter()
    .flat_map(|file| Reader::open(file, 0).unwrap())
    .map(|entry| {
      let Record::V1(record) = entry.unwrap().record;
      assert_eq!(
        (record.instance.as_str(), record.connection),
        ("backfill-t", 1)
      );
      record.body
    })
    .collect()
}

#[::tokio::test]
async fn fills_a_gap_page_by_page() {
  let root = TempRoot::new();
  let source = Fake::new(None);
  // 3000..5000 widened by the margin: messages 2500..=5400 are wanted.
  let stats = run(&source, &[gap(3000, 5000)], &options(&root, 50))
    .await
    .unwrap();
  assert_eq!((stats.requests, stats.filled, stats.stopped), (6, 1, None));
  assert_eq!((stats.messages, stats.skipped), (29, 1));

  let bodies = bodies(&root);
  let fetched: Vec<_> = bodies
    .iter()
    .filter_map(|body| match body {
      Body::Fetched { timestamp, .. } => Some(*timestamp),
      _ => None,
    })
    .collect();
  let wanted: Vec<_> = (25..55)
    .rev()
    .map(|i| i * 100)
    .filter(|&t| t != 4200)
    .collect();
  assert_eq!(fetched, wanted);
  // The ranges chain from the widened end back to the widened start.
  let ranges: Vec<_> = bodies
    .iter()
    .filter_map(|body| match body {
      Body::FetchRange { source, from, to } => {
        assert_eq!(source, "fake");
        Some((*from, *to))
      }
      _ => None,
    })
    .collect();
  assert_eq!(
    ranges,
    [
      (5000, 5500),
      (4500, 5000),
      (4000, 4500),
      (3500, 4000),
      (3000, 3500),
      (2500, 3000),
    ]
  );
  assert_eq!(
    *source.asked.lock().unwrap(),
    [5500, 5000, 4500, 4000, 3500, 3000]
  );
}

#[::tokio::test]
async fn stops_at_the_request_budget() {
  let root = TempRoot::new();
  let source = Fake::new(None);
  let stats = run(&source, &[gap(3000, 5000)], &options(&root, 2))
    .await
    .unwrap();
  assert_eq!((stats.requests, stats.filled), (2, 0));
  assert!(stats.stopped.unwrap().contains("max-requests"));
}

#[::tokio::test]
async fn stops_when_the_rate_limit_runs_low() {
  let root = TempRoot::new();
  let source = Fake::new(Some(3));
  let stats = run(
    &source,
    &[gap(3000, 5000), gap(7000, 8000)],
    &options(&root, 50),
  )
  .await
  .unwrap();
  assert_eq!(stats.requests, 1);
  assert!(stats.stopped.unwrap().contains("rate limit"));
  // What was fetched is still journaled, with its range.
  let ranges = bodies(&root)
    .into_iter()
    .filter(|body| matches!(body, Body::FetchRange { .. }))
    .count();
  assert_eq!(ranges, 1);
}

#[::tokio::test]
async fn an_exhausted_source_covers_back_to_the_start() {
  let root = TempRoot::new();
  let source = Fake::new(None);
  // Before the first message: one empty page settles it.
  let stats = run(&source, &[gap(-3000, -2000)], &options(&root, 50))
    .await
    .unwrap();
  assert_eq!((stats.requests, stats.filled, stats.messages), (1, 1, 0));
}

#[test]
fn parses_rustlesearch_responses() {
  let body = r#"{"type":"success","error":null,"data":{"count":2,
    "countLimit":10000,"messages":[
    {"channel":"Destinygg","text":"TINYING","ts":"2026-11-08T16:54:48.711Z",
     "username":"superblanks","searchAfter":1794156888711},
    {"channel":"Destinygg","text":"hi  there","ts":"2016-01-01T23:58:03.000Z",
     "username":"old","searchAfter":1451692683000}]}}"#;
  let page = parse(body).unwrap();
  assert!(!page.exhausted);
  assert_eq!(
    page.messages,
    [
      Message {
        timestamp: 1_794_156_888_711,
        nick: "superblanks".into(),
        text: "TINYING".into(),
      },
      Message {
        timestamp: 1_451_692_683_000,
        nick: "old".into(),
        text: "hi  there".into(),
      },
    ]
  );
  let empty = r#"{"type":"success","data":{"messages":[]}}"#;
  assert!(parse(empty).unwrap().exhausted);
  assert!(parse(r#"{"type":"error"}"#).is_err());
  assert!(parse("<html>").is_err());
}

#[test]
fn parses_timestamps_and_rate_limits() {
  assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
  assert_eq!(
    parse_timestamp("2024-02-29T23:59:59.999Z"),
    Some(1_709_251_199_999)
  );
  assert_eq!(
    parse_timestamp("2026-11-08T09:01:44.6Z"),
    Some(1_794_128_504_600)
  );
  for bad in [
    "2026-11-08T09:01:44.611",
    "2026-13-01T00:00:00Z",
    "x",
    "2026-11-08T09:01Z",
  ] {
    assert_eq!(parse_timestamp(bad), None, "{bad:?}");
  }
  assert_eq!(
    remaining("limit=200, remaining=191, reset=21864"),
    Some(191)
  );
  assert_eq!(remaining("limit=200"), None);
}
