//! Fetching from linkers.ooo. There are no retries or pacing: ask for a
//! page at a time, minding [`Answer::rate_limit`].
//!
//! The `reqwest` client is the caller's: its timeouts, and (with rustls)
//! crypto provider.

use {
  crate::{Error, Kind, Messages, RateLimit, parse},
  ::common::time::rfc3339,
};

/// Where the API is served.
pub const DEFAULT_BASE: &str = "https://linkers.ooo";

/// The most messages a page can have.
pub const MAX_LIMIT: usize = 500;

/// The API, as the tool named by its user agent.
pub struct Api {
  http: ::reqwest::Client,
  user_agent: String,
  base: String,
  session: Option<String>,
}

/// Which messages to ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
  pub message_type: Kind,
  /// `all` mixes in the copy of the rustle archive; none, linkers.ooo's
  /// own log only.
  pub sources: Option<String>,
  /// The newest to include, unix ms: everything in its second is.
  pub end_time: Option<i64>,
  /// At most [`MAX_LIMIT`].
  pub limit: usize,
}

/// A page, with the rate limit it was answered under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
  pub messages: Messages,
  /// From the `X-RateLimit-*` headers, if there were any.
  pub rate_limit: Option<RateLimit>,
}

impl Api {
  /// As `user_agent`, which names the tool, such as `ledgerline/0.1.0`.
  pub fn new(http: ::reqwest::Client, user_agent: impl Into<String>) -> Self {
    Self {
      http,
      user_agent: user_agent.into(),
      base: DEFAULT_BASE.into(),
      session: None,
    }
  }

  /// Served at `base` instead of [`DEFAULT_BASE`], as in tests.
  pub fn with_base(mut self, base: impl Into<String>) -> Self {
    self.base = base.into();
    self
  }

  /// As a logged-in user, by their `lk_session` cookie, which lifts the
  /// rate limit.
  pub fn with_session(mut self, session: Option<String>) -> Self {
    self.session = session;
    self
  }

  /// `GET /api/messages`: a page of messages, newest first.
  pub async fn messages(&self, query: &Query) -> Result<Answer, Error> {
    let mut params = vec![
      ("messageType", query.message_type.as_str().to_owned()),
      ("limit", query.limit.to_string()),
    ];
    if let Some(sources) = &query.sources {
      params.push(("sources", sources.clone()));
    }
    if let Some(end_time) = query.end_time {
      params.push(("endTime", rfc3339(end_time)));
    }
    let mut request = self
      .http
      .get(format!("{}/api/messages", self.base))
      .header(::reqwest::header::USER_AGENT, &self.user_agent)
      .query(&params);
    if let Some(session) = &self.session {
      request = request.header("cookie", format!("lk_session={session}"));
    }
    let response = request.send().await.map_err(Error::Http)?;
    let rate_limit =
      RateLimit::from_headers(response.headers().iter().filter_map(
        |(name, value)| Some((name.as_str(), value.to_str().ok()?)),
      ));
    let status = response.status();
    let body = response.text().await.map_err(Error::Http)?;
    if !status.is_success() {
      return Err(Error::Status {
        status: status.as_u16(),
        body,
      });
    }
    Ok(Answer {
      messages: parse(&body)?,
      rate_limit,
    })
  }
}
