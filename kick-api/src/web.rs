//! `web.kick.com`'s unofficial API, which kick.com's own pages use: for now
//! only a channel's chat history.
//!
//! It's undocumented, and Kick's terms forbid using it (see
//! `planning/kick-api.md`): it may change or be blocked without notice.
//! Every request names the tool with an honest `User-Agent`; without one,
//! it's refused. There are no retries or pacing: ask for a page at a time.
//!
//! The `reqwest` client is the caller's: its timeouts, and (with rustls)
//! crypto provider.

use {
  crate::{
    realtime::event::User,
    rest::{self, Rest},
    time,
  },
  ::serde::{Deserialize, Deserializer},
  ::serde_json::Value,
  ::std::{error, fmt},
};

/// Where the API is served.
pub const DEFAULT_BASE: &str = "https://web.kick.com/api/v1";

/// The API, as the tool named by its user agent.
pub struct Web {
  http: ::reqwest::Client,
  user_agent: String,
  base: String,
}

impl Web {
  /// As `user_agent`, which names the tool, such as `kick-mod/0.1.0`.
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

  /// `GET /chat/<channel_id>/history`: a page of the channel's messages
  /// before `cursor` (see [`cursor_at`]), or the newest with none. The
  /// channel id is the channel's, not its chatroom's.
  pub async fn history(
    &self,
    channel_id: u64,
    cursor: Option<&str>,
  ) -> Result<Page, Error> {
    let url = format!("{}/chat/{channel_id}/history", self.base);
    let mut request = self
      .http
      .get(url)
      .header(::reqwest::header::USER_AGENT, &self.user_agent)
      .header(::reqwest::header::ACCEPT, "application/json");
    if let Some(cursor) = cursor {
      request = request.query(&[("cursor", cursor)]);
    }
    let response = request.send().await.map_err(Error::Http)?;
    let status = response.status();
    let body = response.text().await.map_err(Error::Http)?;
    if !status.is_success() {
      return Err(Error::Status {
        status: status.as_u16(),
        body,
      });
    }
    match ::serde_json::from_str::<Envelope>(&body) {
      Ok(envelope) => Ok(envelope.data),
      Err(e) => Err(Error::Decode {
        raw: body,
        error: e.to_string(),
      }),
    }
  }
}

/// The cursor for the messages before `ms` (unix ms): Kick's cursors are
/// unix microseconds.
pub fn cursor_at(ms: i64) -> String {
  (ms * 1000).to_string()
}

#[derive(Deserialize)]
struct Envelope {
  data: Page,
}

/// A page of a channel's messages.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Page {
  /// Newest first.
  pub messages: Vec<Message>,
  /// For the page before this one; `None` at the start of the history.
  #[serde(deserialize_with = "nonempty")]
  pub cursor: Option<String>,
  pub pinned_message: Option<Value>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A message in the history: as the realtime `ChatMessageEvent`'s, but for
/// `metadata`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(remote = "Self")]
pub struct Message {
  /// A uuid, the realtime message's `id`.
  pub id: String,
  /// The channel's id.
  pub chat_id: Option<u64>,
  /// The sender's id.
  pub user_id: Option<u64>,
  /// Emotes inline as `[emote:<id>:<name>]`.
  pub content: String,
  /// `message` or `reply`.
  pub r#type: Option<String>,
  /// Seen as JSON in a string.
  pub metadata: Option<Value>,
  /// Unix ms. A string that isn't RFC 3339 stays in `rest`.
  #[serde(skip)]
  pub created_at: Option<i64>,
  pub sender: User,
  pub thread_parent_id: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl<'de> Deserialize<'de> for Message {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    let mut decoded = Self::deserialize(deserializer)?;
    decoded.created_at = time::take(&mut decoded.rest, "created_at");
    Ok(decoded)
  }
}

/// A string, with an empty one as none.
fn nonempty<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<Option<String>, D::Error> {
  let text = Option::<String>::deserialize(deserializer)?;
  Ok(text.filter(|text| !text.is_empty()))
}

/// Why a request failed.
#[derive(Debug)]
pub enum Error {
  /// No answer.
  Http(::reqwest::Error),
  /// An answer that wasn't a success, kept as served.
  Status { status: u16, body: String },
  /// A success that didn't decode, kept as served.
  Decode { raw: String, error: String },
}

impl Error {
  /// The HTTP status, if there was an answer that wasn't a success.
  pub fn status(&self) -> Option<u16> {
    match self {
      Self::Status { status, .. } => Some(*status),
      _ => None,
    }
  }

  /// A 403 from Kick's security policy, which refuses some clients: one
  /// without a `User-Agent`, say.
  pub fn blocked(&self) -> bool {
    matches!(
      self,
      Self::Status { status: 403, body }
        if body.contains("blocked by security policy")
    )
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::Http(e) => write!(f, "request failed: {e}"),
      Self::Status { status, body } => write!(f, "answered {status}: {body}"),
      Self::Decode { error, .. } => write!(f, "didn't decode: {error}"),
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::Http(e) => Some(e),
      _ => None,
    }
  }
}
