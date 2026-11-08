// Check the logger's dedupe key: record live MSGs, then open a second
// connection and compare its HISTORY against what was recorded.
// Usage: cargo run -p dgg-chat --example history_check -- [seconds]
use std::collections::HashMap;
use std::time::Duration;

use dgg_chat::{Client, Item, Options};
use serde_json::Value;

#[tokio::main]
async fn main() {
  let secs = std::env::args()
    .nth(1)
    .and_then(|s| s.parse().ok())
    .unwrap_or(60);
  let mut live = Client::spawn(Options::default());
  let mut seen = Vec::new();
  let deadline = tokio::time::sleep(Duration::from_secs(secs));
  tokio::pin!(deadline);
  println!("recording live MSGs for {secs}s...");
  loop {
    tokio::select! {
      _ = &mut deadline => break,
      item = live.next() => {
        if let Some(Item::Frame(f)) = item
          && f.conn == 1
          && f.event() == "MSG"
        {
          seen.push(f.payload().to_owned());
        }
      }
    }
  }

  let mut second = Client::spawn(Options::default());
  let history = loop {
    if let Some(Item::Frame(f)) = second.next().await
      && f.event() == "HISTORY"
    {
      break f.payload().to_owned();
    }
  };
  let history: Value = serde_json::from_str(&history).expect("HISTORY json");
  let entries = history.as_array().expect("HISTORY is an array");
  println!(
    "HISTORY: {} entries; first: {:.120}",
    entries.len(),
    entries[0]
  );

  // Entries are either "MSG {...}" strings or objects; keep MSG payloads.
  let hist: Vec<String> = entries
    .iter()
    .filter_map(|e| match e {
      Value::String(s) => s.strip_prefix("MSG ").map(str::to_owned),
      v => Some(v.to_string()),
    })
    .collect();
  let key = |raw: &str| {
    let v: Value = serde_json::from_str(raw).unwrap();
    (
      v["timestamp"].as_i64(),
      v["nick"].clone(),
      v["data"].clone(),
    )
  };
  let hist_raw: HashMap<&str, ()> =
    hist.iter().map(|s| (s.as_str(), ())).collect();
  let hist_keys: HashMap<_, &String> =
    hist.iter().map(|s| (key(s), s)).collect();
  let oldest = hist.iter().filter_map(|s| key(s).0).min().unwrap_or(0);

  let in_range: Vec<&String> =
    seen.iter().filter(|s| key(s).0 >= Some(oldest)).collect();
  let raw_hits = in_range
    .iter()
    .filter(|s| hist_raw.contains_key(s.as_str()));
  let key_hits: Vec<_> = in_range
    .iter()
    .filter_map(|s| hist_keys.get(&key(s)).map(|h| (s, h)))
    .collect();
  println!(
    "live MSGs: {}, within HISTORY range: {}, raw-identical: {}, key match: {}",
    seen.len(),
    in_range.len(),
    raw_hits.count(),
    key_hits.len()
  );
  for (live, hist) in key_hits.iter().filter(|(l, h)| l != h).take(3) {
    println!("key match but raw differs:\n  live: {live}\n  hist: {hist}");
  }
  for s in in_range
    .iter()
    .filter(|s| !hist_keys.contains_key(&key(s)))
    .take(3)
  {
    println!("live MSG missing from HISTORY: {s:.200}");
  }
}
