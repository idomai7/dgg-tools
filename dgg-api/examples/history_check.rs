// Check the logger'payload dedupe key: record live MSGs, then open a second
// connection and compare its HISTORY against what was recorded.
// Usage: cargo run -p dgg-api --example history_check -- [seconds]
use {
  ::dgg_api::chat::{Client, Item, Options},
  ::serde_json::Value,
  ::std::{collections::HashMap, time::Duration},
};

#[::tokio::main]
async fn main() {
  let seconds = ::std::env::args()
    .nth(1)
    .and_then(|arg| arg.parse().ok())
    .unwrap_or(60);
  let mut live = Client::spawn(Options::default());
  let mut seen = Vec::new();
  let deadline = ::tokio::time::sleep(Duration::from_secs(seconds));
  ::tokio::pin!(deadline);
  println!("recording live MSGs for {seconds}payload...");
  loop {
    ::tokio::select! {
      _ = &mut deadline => break,
      item = live.next() => {
        if let Some(Item::Frame(frame)) = item
          && frame.connection == 1
          && frame.event() == "MSG"
        {
          seen.push(frame.payload().to_owned());
        }
      }
    }
  }

  let mut second = Client::spawn(Options::default());
  let history = loop {
    if let Some(Item::Frame(frame)) = second.next().await
      && frame.event() == "HISTORY"
    {
      break frame.payload().to_owned();
    }
  };
  let history: Value = ::serde_json::from_str(&history).expect("HISTORY json");
  let entries = history.as_array().expect("HISTORY is an array");
  println!(
    "HISTORY: {} entries; first: {:.120}",
    entries.len(),
    entries[0]
  );

  // Entries are either "MSG {...}" strings or objects; keep MSG payloads.
  let historic: Vec<String> = entries
    .iter()
    .filter_map(|e| match e {
      Value::String(payload) => payload.strip_prefix("MSG ").map(str::to_owned),
      value => Some(value.to_string()),
    })
    .collect();
  let key = |raw: &str| {
    let value: Value = ::serde_json::from_str(raw).unwrap();
    (
      value["timestamp"].as_i64(),
      value["nick"].clone(),
      value["data"].clone(),
    )
  };
  let historic_raw: HashMap<&str, ()> = historic
    .iter()
    .map(|payload| (payload.as_str(), ()))
    .collect();
  let historic_keys: HashMap<_, &String> = historic
    .iter()
    .map(|payload| (key(payload), payload))
    .collect();
  let oldest = historic
    .iter()
    .filter_map(|payload| key(payload).0)
    .min()
    .unwrap_or(0);

  let in_range: Vec<&String> = seen
    .iter()
    .filter(|payload| key(payload).0 >= Some(oldest))
    .collect();
  let raw_hits = in_range
    .iter()
    .filter(|payload| historic_raw.contains_key(payload.as_str()));
  let key_hits: Vec<_> = in_range
    .iter()
    .filter_map(|payload| {
      historic_keys
        .get(&key(payload))
        .map(|historic| (payload, historic))
    })
    .collect();
  println!(
    "live MSGs: {}, within HISTORY range: {}, raw-identical: {}, key match: {}",
    seen.len(),
    in_range.len(),
    raw_hits.count(),
    key_hits.len()
  );
  for (live, historic) in key_hits
    .iter()
    .filter(|(live, historic)| live != historic)
    .take(3)
  {
    println!(
      "key match but raw differs:\n  live: {live}\n  historic: {historic}"
    );
  }
  for payload in in_range
    .iter()
    .filter(|payload| !historic_keys.contains_key(&key(payload)))
    .take(3)
  {
    println!("live MSG missing from HISTORY: {payload:.200}");
  }
}
