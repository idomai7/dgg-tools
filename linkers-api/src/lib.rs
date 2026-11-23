//! linkers.ooo's API, a public log of dgg chat since 2026-01-06, plus its
//! copy of the rustle archive before that: lightweight, so any tool can
//! depend on it.
//!
//! ```http
//! GET <base>/api/messages?messageType=MSG&sources=all&endTime=<ISO 8601>
//!   &limit=500
//! ```
//!
//! returns up to `limit` messages at or before `endTime`, newest first:
//!
//! ```js
//! {
//!   "messages": [
//!     {
//!       "id", "timestamp", "user", "message", "message_type", "channel",
//!       "source", "user_flairs", "subscription_tier",
//!       "subscription_tier_label"
//!     }
//!   ],
//!   "total", "totalCapped", "limit", "offset"
//! }
//! ```
//!
//! Every [`Kind`] of message has the same fields. `endTime` is truncated to
//! the second, and so are the timestamps of linkers.ooo's own messages.
//! Nicks keep their case; newlines in text are spaces. Anonymous requests
//! are limited to 60 a minute per IP (`X-RateLimit-*`); requests with a
//! logged-in user's `lk_session` cookie aren't limited.
//!
//! `sources=all` mixes in the rustle archive's messages that linkers.ooo's
//! own log doesn't have, with an id starting `a` and `source`
//! `rustlesearch` (to the millisecond) or `overrustle` (whole seconds).
//!
//! Decoding needs no feature; fetching, [`Api`], is feature `http`.
//! Timestamps are unix milliseconds, from [`common::time`].

#[cfg(feature = "http")]
mod api;
pub mod rest;

#[cfg(feature = "http")]
pub use api::{Answer, Api, DEFAULT_BASE, MAX_LIMIT, Query};
pub use rest::Rest;

use {
  ::common::time::unix_ms,
  ::serde::{Deserialize, Deserializer, de},
  ::std::{error, fmt},
};

/// A page of messages.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Messages {
  /// Newest first.
  pub messages: Vec<Message>,
  /// How many match, at most a cap linkers.ooo sets.
  pub total: Option<u64>,
  /// Whether `total` was capped.
  pub total_capped: Option<bool>,
  pub limit: Option<u64>,
  pub offset: Option<u64>,
  /// Such as `sources`, with `sources=all`.
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A message in the log: said in chat, or what chat announced.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Message {
  pub id: Id,
  /// dgg's timestamp, unix ms: to the second for linkers.ooo's own
  /// messages.
  #[serde(deserialize_with = "timestamp")]
  pub timestamp: i64,
  /// Who said it; for a ban or mute, the moderator, and for a
  /// subscription, gift or donation, `Subscriber`.
  pub user: String,
  /// What was said, or what chat announced (see [`Kind`]).
  pub message: String,
  pub message_type: Kind,
  /// `Destinygg`.
  pub channel: Option<String>,
  /// Where it was logged: absent or `linkers` for linkers.ooo's own log,
  /// `rustlesearch` or `overrustle` for its copy of the rustle archive.
  pub source: Option<String>,
  /// The sender's flairs, as dgg names them: for a message, ban or mute.
  pub user_flairs: Option<Vec<String>>,
  /// The tier: for a message, the sender's, if subscribed, and for a
  /// subscription or gift, the one bought.
  pub subscription_tier: Option<u64>,
  /// The tier's name.
  pub subscription_tier_label: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A message's id.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Id {
  /// linkers.ooo's own messages'.
  Number(u64),
  /// From its copy of the rustle archive: `a<number>`.
  Text(String),
}

/// What kind of message it is, its `message_type`. Ban and mute messages
/// read `<nick> banned by <moderator>` (or `muted by`); the rest are chat's
/// announcement, such as `<nick> has resubscribed! Active for 3 months.`
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
  /// Said in chat.
  Msg,
  Ban,
  Mute,
  Subscription,
  GiftSub,
  MassGift,
  Donation,
  /// One this crate doesn't know, as served.
  Other(String),
}

impl Kind {
  /// As linkers.ooo names it, as in `messageType`.
  pub fn as_str(&self) -> &str {
    match self {
      Self::Msg => "MSG",
      Self::Ban => "BAN",
      Self::Mute => "MUTE",
      Self::Subscription => "SUBSCRIPTION",
      Self::GiftSub => "GIFTSUB",
      Self::MassGift => "MASSGIFT",
      Self::Donation => "DONATION",
      Self::Other(name) => name,
    }
  }

  /// The kind linkers.ooo names `name`.
  pub fn named(name: &str) -> Self {
    match name {
      "MSG" => Self::Msg,
      "BAN" => Self::Ban,
      "MUTE" => Self::Mute,
      "SUBSCRIPTION" => Self::Subscription,
      "GIFTSUB" => Self::GiftSub,
      "MASSGIFT" => Self::MassGift,
      "DONATION" => Self::Donation,
      other => Self::Other(other.into()),
    }
  }
}

impl<'de> Deserialize<'de> for Kind {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    Ok(Self::named(&String::deserialize(deserializer)?))
  }
}

/// Decodes a page of messages as served.
pub fn parse(text: &str) -> Result<Messages, Error> {
  ::serde_json::from_str(text).map_err(|e| Error::Decode {
    raw: text.into(),
    error: e.to_string(),
  })
}

fn timestamp<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<i64, D::Error> {
  let text = String::deserialize(deserializer)?;
  unix_ms(&text)
    .ok_or_else(|| de::Error::custom(format!("bad timestamp {text:?}")))
}

/// The `X-RateLimit-*` headers, each if sent and a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimit {
  /// Requests allowed a minute.
  pub limit: Option<u64>,
  /// Requests left this minute.
  pub remaining: Option<u64>,
  /// When the minute resets, unix seconds.
  pub reset: Option<u64>,
}

impl RateLimit {
  /// From the headers' values, by their names' last part: `Limit`,
  /// `Remaining` or `Reset`.
  pub fn from_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
  ) -> Option<Self> {
    let mut found = None;
    for (name, value) in headers {
      let name = name.to_ascii_lowercase();
      let Some(part) = name.strip_prefix("x-ratelimit-") else {
        continue;
      };
      let found = found.get_or_insert_with(Self::default);
      let value = value.trim().parse().ok();
      match part {
        "limit" => found.limit = value,
        "remaining" => found.remaining = value,
        "reset" => found.reset = value,
        _ => {}
      }
    }
    found
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
