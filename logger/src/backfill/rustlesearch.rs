//! rustlesearch.dev, a public archive of dgg chat.
//!
//! `GET <base>/anon/search?channel=Destinygg&search_after=<ms>` returns up
//! to 100 messages older than `search_after`, newest first:
//!
//! ```js
//! {
//!   "data": {
//!     "messages": [{"channel", "text", "ts", "username", "searchAfter"}]
//!   }
//! }
//! ```
//!
//! `ts` is dgg's timestamp as ISO 8601 UTC, to the millisecond (whole
//! seconds for old logs). Usernames are lowercased and newlines in text are
//! spaces. Requests are limited to 200 a day, reported in a `Ratelimit:
//! limit=200, remaining=191, reset=21864` header.

use {
  super::{Message, Page, Source},
  ::common::time::unix_ms,
  ::serde_json::Value,
};

pub const BASE_URL: &str = "https://api-v2.rustlesearch.dev";

pub struct Rustlesearch {
  client: ::reqwest::Client,
  base_url: String,
  channel: String,
}

impl Rustlesearch {
  pub fn new(base_url: impl Into<String>, channel: impl Into<String>) -> Self {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let client = ::reqwest::Client::builder()
      .user_agent(format!("dgg-tools-logger/{}", crate::BUILD))
      .build()
      .expect("http client");
    Self {
      client,
      base_url: base_url.into(),
      channel: channel.into(),
    }
  }
}

impl Source for Rustlesearch {
  fn name(&self) -> &str {
    "rustlesearch"
  }

  async fn page(&self, before: i64) -> Result<Page, String> {
    let response = self
      .client
      .get(format!("{}/anon/search", self.base_url))
      .query(&[
        ("channel", self.channel.as_str()),
        ("search_after", &before.to_string()),
      ])
      .send()
      .await
      .map_err(|e| e.to_string())?;
    let remaining = response
      .headers()
      .get("ratelimit")
      .and_then(|value| value.to_str().ok())
      .and_then(remaining);
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
      return Err(format!("HTTP {status}: {body:.200}"));
    }
    let mut page = parse(&body)?;
    page.remaining = remaining;
    Ok(page)
  }
}

/// Parses a search response body.
pub fn parse(body: &str) -> Result<Page, String> {
  let value: Value =
    ::serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
  let Some(items) = value["data"]["messages"].as_array() else {
    return Err(format!("no data.messages: {body:.200}"));
  };
  let mut messages = Vec::with_capacity(items.len());
  for item in items {
    let field =
      |key| item[key].as_str().ok_or(format!("message without {key}"));
    let timestamp = field("ts")?;
    messages.push(Message {
      timestamp: unix_ms(timestamp).ok_or(format!("bad ts {timestamp:?}"))?,
      nick: field("username")?.into(),
      text: field("text")?.into(),
      origin: None,
    });
  }
  Ok(Page {
    exhausted: messages.is_empty(),
    messages,
    remaining: None,
  })
}

/// `remaining` from a `Ratelimit: limit=200, remaining=191, reset=21864`
/// header.
pub fn remaining(header: &str) -> Option<u64> {
  header
    .split(',')
    .find_map(|part| part.trim().strip_prefix("remaining=")?.parse().ok())
}
