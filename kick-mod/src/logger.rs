//! The logger's `serve`, which kick-mod reads Kick from: each channel's
//! live chat (see [`chat::logger`](crate::chat::logger)), and a user's
//! history in a channel (see [`history`](crate::history)), with its query
//! token.

use {
  crate::{chat, config::Logger},
  ::common::config::Secret,
  ::reqwest::{
    Certificate, RequestBuilder, StatusCode,
    header::{ACCEPT, HeaderValue},
  },
  ::std::time::Duration,
};

pub const FIRST_RETRY: Duration = Duration::from_secs(1);

/// How long a user's history may take.
const LOOKUP: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Client {
  http: ::reqwest::Client,
  /// Without a trailing `/`.
  url: String,
  token: Secret,
  /// How long after a stream drops it's first retried.
  pub(crate) first_retry: Duration,
}

impl Client {
  pub fn new(logger: &Logger) -> Result<Self, String> {
    let mut builder =
      ::reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
    if let Some(ca) = &logger.ca {
      let certs = Certificate::from_pem_bundle(ca.as_bytes())
        .map_err(|e| format!("logger.ca: can't read it: {e}"))?;
      builder = builder.tls_certs_only(certs).https_only(true);
    }
    Ok(Self {
      http: builder.build().map_err(|e| format!("logger: {e}"))?,
      url: logger.url.trim_end_matches('/').into(),
      token: logger.token.clone(),
      first_retry: FIRST_RETRY,
    })
  }

  /// Retries a dropped stream first after `first_retry`, doubling.
  pub fn with_first_retry(mut self, first_retry: Duration) -> Self {
    self.first_retry = first_retry;
    self
  }

  /// `GET` of `path` under Kick's channel `slug`, which has to be
  /// [journalable](chat::journalable).
  fn get(&self, slug: &str, path: &str) -> RequestBuilder {
    let url = format!("{}/kick/v0/channels/{slug}/{path}", self.url);
    self.http.get(url).bearer_auth(self.token.expose())
  }

  /// Channel `slug`'s live chat, as server-sent events.
  pub(crate) fn events(&self, slug: &str) -> RequestBuilder {
    self
      .get(slug, "events")
      .header(ACCEPT, HeaderValue::from_static("text/event-stream"))
  }

  /// The history of `user_id` in channel `slug`, as the logger answers
  /// it (JSON); none if the logger has no such channel.
  pub async fn user(
    &self,
    slug: &str,
    user_id: u64,
  ) -> Result<Option<String>, String> {
    if !chat::journalable(slug) {
      return Ok(None);
    }
    let request = self.get(slug, &format!("users/{user_id}")).timeout(LOOKUP);
    let response = request.send().await.map_err(|e| e.to_string())?;
    match response.status() {
      StatusCode::NOT_FOUND => Ok(None),
      status if status.is_success() => {
        Ok(Some(response.text().await.map_err(|e| e.to_string())?))
      }
      status => Err(format!("answered {status}")),
    }
  }
}
