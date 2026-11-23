//! linkers.ooo, a public log of dgg chat since 2026-01-06, plus its copy of
//! the rustle archive before that, through `linkers_api`: pages of up to
//! 500 messages, newest first, with nicks in their case.
//!
//! linkers.ooo's own messages have timestamps truncated to the second.
//! Pages ask for `sources=all`, which mixes in the rustle archive's
//! messages that its own log doesn't have, with `source` `rustlesearch` or
//! `overrustle`; those are journaled as `linkers-rustlesearch` and
//! `linkers-overrustle`.
//!
//! A full page usually ends partway through a second, and asking for what's
//! before that second would skip the rest of it, so the oldest second of a
//! full page is left for the next one.

use {
  super::{Message, Page, Source},
  ::linkers_api::{Api, Kind, MAX_LIMIT, Messages, Query},
  ::logger_journal::record::v0::valid_instance,
};

pub struct Linkers {
  api: Api,
}

impl Linkers {
  pub fn new(api: Api) -> Self {
    Self { api }
  }
}

impl Source for Linkers {
  fn name(&self) -> &str {
    "linkers"
  }

  async fn page(&self, before: i64) -> Result<Page, String> {
    // The last whole second before `before`; it's included.
    let end = (before - 1).div_euclid(1000) * 1000;
    let query = Query {
      message_type: Kind::Msg,
      sources: Some("all".into()),
      end_time: Some(end),
      limit: MAX_LIMIT,
    };
    let answer = self.api.messages(&query).await.map_err(|e| e.to_string())?;
    let mut page = page(answer.messages, MAX_LIMIT)?;
    page.remaining = answer.rate_limit.and_then(|limit| limit.remaining);
    // `endTime` takes in the rest of its second.
    page.messages.retain(|message| message.timestamp < before);
    Ok(page)
  }
}

/// A page of messages asked for `limit` at a time, as backfill's page: what
/// was said, by the archive it came from. A full page loses its oldest
/// second, unless that would leave nothing.
pub fn page(answer: Messages, limit: usize) -> Result<Page, String> {
  let exhausted = answer.messages.len() < limit;
  let mut messages = Vec::with_capacity(answer.messages.len());
  for message in answer.messages {
    if message.message_type != Kind::Msg {
      continue;
    }
    let origin = match message.source.as_deref() {
      None | Some("linkers") => None,
      // Any other archive it may add is kept apart the same way.
      Some(other) if valid_instance(other) => Some(format!("linkers-{other}")),
      Some(other) => return Err(format!("bad source {other:?}")),
    };
    messages.push(Message {
      timestamp: message.timestamp,
      nick: message.user,
      text: message.message,
      origin,
    });
  }
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
