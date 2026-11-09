//! linkers.ooo, a public log of dgg chat since 2026-01-06, plus its copy of
//! the rustle archive before that.
//!
//! `GET <base>/api/messages?messageType=MSG&sources=all&endTime=<ISO 8601>
//! &limit=500` returns `{"messages": [{"id", "timestamp", "user", "message",
//! "message_type", "source", ...}], "total", "totalCapped", "limit",
//! "offset"}`: up to `limit` messages at or before `endTime`, newest first.
//! `endTime` is truncated to the second. Nicks keep their case; newlines in
//! text are spaces. Anonymous requests are limited to 60 a minute per IP
//! (`X-RateLimit-Remaining`); requests with a logged-in user's `lk_session`
//! cookie aren't limited.
//!
//! linkers.ooo's own messages (`source` absent or `linkers`) have
//! timestamps truncated to the second. `sources=all` mixes in the rustle
//! archive's messages that linkers.ooo's own log doesn't have, with an id
//! starting `a` and `source` `rustlesearch` (to the millisecond) or
//! `overrustle` (whole seconds); those are journaled as
//! `linkers-rustlesearch` and `linkers-overrustle`.
//!
//! A full page usually ends partway through a second, and asking for what's
//! before that second would skip the rest of it, so the oldest second of a
//! full page is left for the next one.

use {
  super::{Message, Page, Source, rustlesearch::parse_timestamp},
  ::journal::{layout::utc_day, record::v0::valid_instance},
  ::serde_json::Value,
};

pub const BASE_URL: &str = "https://linkers.ooo";

/// Messages asked for per page, the most the API allows.
pub const PAGE_SIZE: usize = 500;

pub struct Linkers {
  client: ::reqwest::Client,
  base_url: String,
  /// An `lk_session` cookie, to lift the rate limit.
  session: Option<String>,
}

impl Linkers {
  pub fn new(base_url: impl Into<String>, session: Option<String>) -> Self {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let client = ::reqwest::Client::builder()
      .user_agent(format!("dgg-tools-logger/{}", crate::BUILD))
      .build()
      .expect("http client");
    Self {
      client,
      base_url: base_url.into(),
      session,
    }
  }
}

impl Source for Linkers {
  fn name(&self) -> &str {
    "linkers"
  }

  async fn page(&self, before: i64) -> Result<Page, String> {
    // The last whole second before `before`; it's included.
    let end = (before - 1).div_euclid(1000) * 1000;
    let mut request = self
      .client
      .get(format!("{}/api/messages", self.base_url))
      .query(&[
        ("messageType", "MSG"),
        ("sources", "all"),
        ("endTime", &format_timestamp(end)),
        ("limit", &PAGE_SIZE.to_string()),
      ]);
    if let Some(session) = &self.session {
      request = request.header("cookie", format!("lk_session={session}"));
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let remaining = response
      .headers()
      .get("x-ratelimit-remaining")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.trim().parse().ok());
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
      return Err(format!("HTTP {status}: {body:.200}"));
    }
    let mut page = parse(&body, PAGE_SIZE)?;
    page.remaining = remaining;
    // `endTime` takes in the rest of its second.
    page.messages.retain(|message| message.timestamp < before);
    Ok(page)
  }
}

/// Parses a response body for a request of `limit` messages. A full page
/// loses its oldest second, unless that would leave nothing.
pub fn parse(body: &str, limit: usize) -> Result<Page, String> {
  let value: Value =
    ::serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
  let Some(items) = value["messages"].as_array() else {
    return Err(format!("no messages: {body:.200}"));
  };
  let mut messages = Vec::with_capacity(items.len());
  for item in items {
    let field =
      |key| item[key].as_str().ok_or(format!("message without {key}"));
    if field("message_type")? != "MSG" {
      continue;
    }
    let timestamp = field("timestamp")?;
    let origin = match item["source"].as_str() {
      None | Some("linkers") => None,
      // Any other archive it may add is kept apart the same way.
      Some(other) if valid_instance(other) => Some(format!("linkers-{other}")),
      Some(other) => return Err(format!("bad source {other:?}")),
    };
    messages.push(Message {
      timestamp: parse_timestamp(timestamp)
        .ok_or(format!("bad timestamp {timestamp:?}"))?,
      nick: field("user")?.into(),
      text: field("message")?.into(),
      origin,
    });
  }
  let exhausted = items.len() < limit;
  if !exhausted && let Some(oldest) = messages.last().map(|m| m.timestamp) {
    let second = oldest.div_euclid(1000);
    let kept = messages
      .iter()
      .rposition(|message| message.timestamp.div_euclid(1000) != second)
      .map_or(0, |last| last + 1);
    if kept > 0 {
      messages.truncate(kept);
    }
  }
  Ok(Page {
    messages,
    exhausted,
    remaining: None,
  })
}

/// `YYYY-MM-DDTHH:MM:SS.fffZ` for unix ms.
pub fn format_timestamp(ms: i64) -> String {
  let in_day = ms.rem_euclid(86_400_000);
  let (hours, minutes) = (in_day / 3_600_000, in_day / 60_000 % 60);
  let (seconds, millis) = (in_day / 1000 % 60, in_day % 1000);
  format!(
    "{}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z",
    utc_day(ms)
  )
}
