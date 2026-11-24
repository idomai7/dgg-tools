//! The unofficial channel lookup, `GET kick.com/api/v2/channels/<slug>`.
//!
//! Kick's official API has no chatroom ids, and chat is read by chatroom.
//! This decodes the answer, and feature `lookup`'s [`look_up`] asks.
//! Cloudflare refuses some clients here: reqwest without a `User-Agent`,
//! or with curl's, as it does on `web.kick.com`. So the asking tool names
//! itself, by its codename.

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

/// Asks the lookup for `slug` with `http`, anonymously, as `user_agent`,
/// which names the tool (such as `ledgerline/0.1.0+a3c579a14414`), waiting
/// at most 20 seconds. A failure says why, with the start of what was
/// answered.
#[cfg(feature = "lookup")]
pub async fn look_up(
  http: &::reqwest::Client,
  user_agent: &str,
  slug: &str,
) -> Result<Channel, String> {
  use ::reqwest::header::{ACCEPT, USER_AGENT};
  let response = http
    .get(url(slug))
    .header(USER_AGENT, user_agent)
    .header(ACCEPT, "application/json")
    .timeout(::std::time::Duration::from_secs(20))
    .send()
    .await
    .map_err(|e| e.to_string())?;
  let status = response.status();
  let body = response.text().await.map_err(|e| e.to_string())?;
  let start = start(&body);
  if !status.is_success() {
    return Err(format!("answered {status}: {start}"));
  }
  parse(&body).map_err(|e| format!("{e}: {start}"))
}

/// The start of `body`, enough to say what it is: a refusal's page can be
/// long.
#[cfg(feature = "lookup")]
fn start(body: &str) -> &str {
  let body = body.trim();
  match body.char_indices().nth(300) {
    Some((at, _)) => &body[..at],
    None => body,
  }
}
