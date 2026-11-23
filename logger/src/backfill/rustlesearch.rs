//! rustlesearch.dev, a public archive of dgg chat, through
//! `rustlesearch_api`: pages of up to 100 messages, newest first, with
//! nicks lowercased. Requests are limited to 200 a day.

use {
  super::{Message, Page, Source},
  ::rustlesearch_api::{Api, Search},
};

pub struct Rustlesearch {
  api: Api,
  channel: String,
}

impl Rustlesearch {
  pub fn new(api: Api, channel: impl Into<String>) -> Self {
    Self {
      api,
      channel: channel.into(),
    }
  }
}

impl Source for Rustlesearch {
  fn name(&self) -> &str {
    "rustlesearch"
  }

  async fn page(&self, before: i64) -> Result<Page, String> {
    let answer = self
      .api
      .search(&self.channel, before)
      .await
      .map_err(|e| e.to_string())?;
    let mut page = page(answer.search);
    page.remaining = answer.rate_limit.and_then(|limit| limit.remaining);
    Ok(page)
  }
}

/// A search's messages, as a page; with none, the archive is exhausted.
pub fn page(search: Search) -> Page {
  let messages: Vec<_> = search
    .messages
    .into_iter()
    .map(|message| Message {
      timestamp: message.ts,
      nick: message.username,
      text: message.text,
      origin: None,
    })
    .collect();
  Page {
    exhausted: messages.is_empty(),
    messages,
    remaining: None,
  }
}
