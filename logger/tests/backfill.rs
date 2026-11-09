use {
  ::journal::{
    Reader, Record,
    layout::{backfill_files, live_files},
    record::v0::{self, Body},
  },
  ::logger::backfill::{
    Message, Options, Page, Source, linkers, run,
    rustlesearch::{parse, parse_timestamp, remaining},
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
  /// Requests left to fail.
  failing: AtomicU32,
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
        origin: (i == 30).then(|| "elsewhere".into()),
      })
      .collect();
    Self {
      messages,
      remaining,
      asked: Mutex::new(Vec::new()),
      failing: AtomicU32::new(0),
    }
  }
}

impl Source for Fake {
  fn name(&self) -> &str {
    "fake"
  }

  async fn page(&self, before: i64) -> Result<Page, String> {
    self.asked.lock().unwrap().push(before);
    if self
      .failing
      .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
      .is_ok()
    {
      return Err("down".into());
    }
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

/// A gap's range, widened by 500 ms each side as the bin does.
fn gap(start: i64, end: i64) -> (i64, i64) {
  (start - 500, end + 500)
}

fn options(root: &TempRoot, max_requests: u64) -> Options {
  Options {
    journal_root: root.0.join("journal"),
    instance: "backfill-t".into(),
    max_requests,
    reserve: 10,
    delay: Duration::ZERO,
    retries: 0,
    report: None,
  }
}

/// Everything journaled, which must all be in backfill files.
fn bodies(root: &TempRoot) -> Vec<v0::Body> {
  let journal = root.0.join("journal");
  assert!(live_files(&journal).unwrap().is_empty());
  backfill_files(&journal)
    .unwrap()
    .iter()
    .flat_map(|file| Reader::open(file, 0).unwrap())
    .map(|entry| {
      let Record::V0(record) = entry.unwrap().record;
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
  // A relayed message is journaled under its origin.
  let origins: Vec<_> = bodies
    .iter()
    .filter_map(|body| match body {
      Body::Fetched {
        source, timestamp, ..
      } if source != "fake" => Some((source.as_str(), *timestamp)),
      _ => None,
    })
    .collect();
  assert_eq!(origins, [("elsewhere", 3000)]);
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

#[::tokio::test(start_paused = true)]
async fn retries_failed_requests() {
  let root = TempRoot::new();
  let source = Fake::new(None);
  source.failing.store(2, Ordering::Relaxed);
  let mut options = options(&root, 50);
  options.retries = 2;
  options.delay = Duration::from_millis(1);
  // Waits 1 s then 2 s, which paused time skips.
  let stats = run(&source, &[gap(3000, 5000)], &options).await.unwrap();
  assert_eq!((stats.retried, stats.filled, stats.stopped), (2, 1, None));
  assert_eq!(stats.messages, 29);

  // One failure too many gives up.
  let root = TempRoot::new();
  source.failing.store(3, Ordering::Relaxed);
  let mut options = self::options(&root, 50);
  options.retries = 2;
  let stats = run(&source, &[gap(3000, 5000)], &options).await.unwrap();
  assert_eq!((stats.retried, stats.filled), (2, 0));
  assert!(stats.stopped.unwrap().contains("down"));
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
        origin: None,
      },
      Message {
        timestamp: 1_451_692_683_000,
        nick: "old".into(),
        text: "hi  there".into(),
        origin: None,
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

#[test]
fn parses_linkers_responses() {
  let item = |id, timestamp: &str, user: &str, kind: &str| {
    ::serde_json::json!({
      "id": id,
      "timestamp": timestamp,
      "user": user,
      "message": format!("{user} {id}"),
      "message_type": kind,
      "channel": "Destinygg",
      "user_flairs": [],
    })
  };
  let body = |items: Vec<::serde_json::Value>| {
    ::serde_json::json!({
      "messages": items,
      "total": 4,
      "totalCapped": false,
      "limit": 4,
      "offset": 0,
    })
    .to_string()
  };
  let listed = |page: &Page| -> Vec<(i64, String)> {
    page
      .messages
      .iter()
      .map(|message| (message.timestamp, message.text.clone()))
      .collect()
  };
  let items = vec![
    item(4, "2026-11-08T20:05:00.000Z", "Spr1ggs", "MSG"),
    item(3, "2026-11-08T20:04:59.000Z", "glommer", "MSG"),
    item(1, "2026-11-08T20:04:57.000Z", "eerie", "MSG"),
    item(2, "2026-11-08T20:04:57.000Z", "Radiant", "MSG"),
  ];
  let at = |seconds: i64| 1_794_168_240_000 + seconds * 1000;

  // A short page is the last; nothing is trimmed.
  let page = linkers::parse(&body(items.clone()), 5).unwrap();
  assert!(page.exhausted);
  assert_eq!(
    listed(&page),
    [
      (at(60), "Spr1ggs 4".into()),
      (at(59), "glommer 3".into()),
      (at(57), "eerie 1".into()),
      (at(57), "Radiant 2".into()),
    ]
  );
  assert_eq!(page.messages[0].nick, "Spr1ggs");

  // A full page leaves its oldest second for the next.
  let page = linkers::parse(&body(items.clone()), 4).unwrap();
  assert!(!page.exhausted);
  assert_eq!(
    listed(&page),
    [(at(60), "Spr1ggs 4".into()), (at(59), "glommer 3".into())]
  );

  // Unless that second is all there is.
  let same = vec![items[2].clone(), items[3].clone()];
  let page = linkers::parse(&body(same), 2).unwrap();
  assert_eq!(page.messages.len(), 2);

  // Other kinds are dropped, but still count toward a full page.
  let mixed = vec![
    items[0].clone(),
    item(9, "2026-11-08T20:04:58.000Z", "Bot", "BAN"),
  ];
  let page = linkers::parse(&body(mixed), 3).unwrap();
  assert!(page.exhausted);
  assert_eq!(listed(&page), [(at(60), "Spr1ggs 4".into())]);

  // Relayed archive rows are tagged with where they came from.
  let mut relayed = item(7, "2025-12-20T12:00:56.123Z", "hesperus", "MSG");
  relayed["id"] = "a274480534".into();
  relayed["source"] = "rustlesearch".into();
  let mut old = item(8, "2019-02-28T23:59:56.000Z", "gorto", "MSG");
  old["source"] = "overrustle".into();
  let mut own = items[0].clone();
  own["source"] = "linkers".into();
  let page = linkers::parse(&body(vec![own, relayed, old.clone()]), 5).unwrap();
  let origins: Vec<_> = page
    .messages
    .iter()
    .map(|message| (message.timestamp % 1000, message.origin.as_deref()))
    .collect();
  assert_eq!(
    origins,
    [
      (0, None),
      (123, Some("linkers-rustlesearch")),
      (0, Some("linkers-overrustle")),
    ]
  );
  old["source"] = "two words".into();
  assert!(linkers::parse(&body(vec![old]), 5).is_err());

  assert!(linkers::parse(r#"{"error":"Failed"}"#, 5).is_err());
  assert!(linkers::parse("<html>", 5).is_err());
  assert_eq!(
    linkers::format_timestamp(at(57) + 5),
    "2026-11-08T20:04:57.005Z"
  );
}
