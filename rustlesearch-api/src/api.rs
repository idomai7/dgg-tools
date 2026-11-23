//! Fetching from rustlesearch.dev. There are no retries or pacing: ask for
//! a page at a time, minding [`Answer::rate_limit`].
//!
//! The `reqwest` client is the caller's: its timeouts, and (with rustls)
//! crypto provider.

use crate::{Error, RateLimit, Search, parse};

/// Where the API is served.
pub const DEFAULT_BASE: &str = "https://api-v2.rustlesearch.dev";

/// The API, as the tool named by its user agent.
pub struct Api {
  http: ::reqwest::Client,
  user_agent: String,
  base: String,
}

/// A search, with the rate limit it was answered under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
  pub search: Search,
  /// From the `Ratelimit` header, if there was one.
  pub rate_limit: Option<RateLimit>,
}

impl Api {
  /// As `user_agent`, which names the tool, such as `ledgerline/0.1.0`.
  pub fn new(http: ::reqwest::Client, user_agent: impl Into<String>) -> Self {
    Self {
      http,
      user_agent: user_agent.into(),
      base: DEFAULT_BASE.into(),
    }
  }

  /// Served at `base` instead of [`DEFAULT_BASE`], as in tests.
  pub fn with_base(mut self, base: impl Into<String>) -> Self {
    self.base = base.into();
    self
  }

  /// `GET /anon/search`: up to 100 of `channel`'s messages before
  /// `search_after` (unix ms), newest first. dgg's channel is `Destinygg`.
  pub async fn search(
    &self,
    channel: &str,
    search_after: i64,
  ) -> Result<Answer, Error> {
    let response = self
      .http
      .get(format!("{}/anon/search", self.base))
      .header(::reqwest::header::USER_AGENT, &self.user_agent)
      .query(&[
        ("channel", channel),
        ("search_after", &search_after.to_string()),
      ])
      .send()
      .await
      .map_err(Error::Http)?;
    let rate_limit = response
      .headers()
      .get("ratelimit")
      .and_then(|value| value.to_str().ok())
      .map(RateLimit::parse);
    let status = response.status();
    let body = response.text().await.map_err(Error::Http)?;
    if !status.is_success() {
      return Err(Error::Status {
        status: status.as_u16(),
        body,
      });
    }
    Ok(Answer {
      search: parse(&body)?,
      rate_limit,
    })
  }
}
