//! rustlesearch.dev's API, a public archive of dgg chat: lightweight, so any
//! tool can depend on it.
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
//!
//! Decoding needs no feature; fetching, [`Api`], is feature `http`.
//! Timestamps are unix milliseconds, from [`common::time`].

#[cfg(feature = "http")]
mod api;
pub mod rest;

#[cfg(feature = "http")]
pub use api::{Answer, Api, DEFAULT_BASE};
pub use rest::Rest;

use {
  ::common::time::unix_ms,
  ::serde::{Deserialize, Deserializer, de},
  ::std::{error, fmt},
};

/// A search's messages.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Search {
  /// Newest first; none before the start of the archive.
  pub messages: Vec<Message>,
  /// Such as `count` and `countLimit`.
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A message in the archive.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
  pub channel: Option<String>,
  /// The nick, lowercased.
  pub username: String,
  /// Newlines as spaces.
  pub text: String,
  /// dgg's timestamp, unix ms.
  #[serde(deserialize_with = "timestamp")]
  pub ts: i64,
  /// The `search_after` for the messages before this one.
  pub search_after: Option<i64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

#[derive(Deserialize)]
struct Envelope {
  data: Search,
}

/// Decodes a search's answer as served.
pub fn parse(text: &str) -> Result<Search, Error> {
  match ::serde_json::from_str::<Envelope>(text) {
    Ok(envelope) => Ok(envelope.data),
    Err(e) => Err(Error::Decode {
      raw: text.into(),
      error: e.to_string(),
    }),
  }
}

fn timestamp<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<i64, D::Error> {
  let text = String::deserialize(deserializer)?;
  unix_ms(&text)
    .ok_or_else(|| de::Error::custom(format!("bad timestamp {text:?}")))
}

/// The `Ratelimit` header: `limit=200, remaining=191, reset=21864`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimit {
  /// Requests allowed in a window.
  pub limit: Option<u64>,
  /// Requests left in this one.
  pub remaining: Option<u64>,
  /// Seconds until the window resets.
  pub reset: Option<u64>,
}

impl RateLimit {
  /// Each part that's there and a number.
  pub fn parse(header: &str) -> Self {
    let part = |name: &str| {
      header.split(',').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.trim().parse().ok())?
      })
    };
    Self {
      limit: part("limit"),
      remaining: part("remaining"),
      reset: part("reset"),
    }
  }
}

/// Why a request failed.
#[derive(Debug)]
pub enum Error {
  /// No answer.
  #[cfg(feature = "http")]
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
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      #[cfg(feature = "http")]
      Self::Http(e) => write!(f, "request failed: {e}"),
      // An error page can be long.
      Self::Status { status, body } => write!(f, "HTTP {status}: {body:.200}"),
      Self::Decode { raw, error } => {
        write!(f, "didn't decode: {error}: {raw:.200}")
      }
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      #[cfg(feature = "http")]
      Self::Http(e) => Some(e),
      _ => None,
    }
  }
}
