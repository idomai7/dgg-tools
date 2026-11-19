//! The unofficial channel lookup, `GET kick.com/api/v2/channels/<slug>`.
//!
//! Kick's official API has no chatroom ids, and chat is read by chatroom.
//! Cloudflare refuses some HTTP clients here (reqwest among them), so this
//! only decodes the answer, and fetching it is left to the tool.

use ::serde::Deserialize;

/// Where a channel is looked up.
pub fn url(slug: &str) -> String {
  format!("https://kick.com/api/v2/channels/{slug}")
}

/// A channel's ids, out of the many fields the lookup answers with.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Channel {
  /// The channel's id.
  pub id: u64,
  /// The broadcaster's user id: the official API's `broadcaster_user_id`.
  pub user_id: u64,
  pub slug: String,
  pub chatroom: Chatroom,
}

/// A channel's chatroom.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Chatroom {
  /// The id its realtime channels are named by: `chatrooms.<id>.v2`.
  pub id: u64,
}

/// Decodes a lookup's answer.
pub fn parse(json: &str) -> Result<Channel, ::serde_json::Error> {
  ::serde_json::from_str(json)
}
