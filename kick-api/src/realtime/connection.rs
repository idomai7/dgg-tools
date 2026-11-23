//! Kick's setup for its realtime socket, named after its endpoints: the
//! [`connection`] endpoint answers which socket to open, and
//! [`auth/connection`](token) the token to connect with. Neither needs a
//! login. Both are asked again for every connection, as the socket's
//! address changes between them.
//!
//! The `reqwest` client is the caller's: its user agent, timeouts, and
//! (with rustls) crypto provider.

use {
  ::serde::Deserialize,
  ::serde_json::json,
  ::std::{error, fmt},
};

/// Where Kick's realtime setup is served.
pub const DEFAULT_BASE: &str = "https://web.kick.com/api/v1/realtime";

/// Why asking failed.
#[derive(Debug)]
pub enum Error {
  /// No answer, or not a success.
  Http(::reqwest::Error),
  /// An answer that didn't decode, kept as served.
  Decode { raw: String, error: String },
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::Http(e) => write!(f, "request failed: {e}"),
      Self::Decode { error, .. } => write!(f, "didn't decode: {error}"),
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::Http(e) => Some(e),
      Self::Decode { .. } => None,
    }
  }
}

/// Asks `base`'s `connection` endpoint which socket to open, as client
/// `client_id` (a uuid).
pub async fn connection(
  http: &::reqwest::Client,
  base: &str,
  client_id: &str,
) -> Result<String, Error> {
  let body = json!({
    "client": { "id": client_id, "type": "web" },
    "capabilities": { "accepted_providers": [{ "provider": "centrifugo" }] },
  });
  let raw = post(http, &format!("{base}/connection"), body).await?;
  decode(raw, decode_connection)
}

/// Asks `base`'s `auth/connection` endpoint for a token to connect with,
/// for the same `client_id` as the socket was asked for.
pub async fn token(
  http: &::reqwest::Client,
  base: &str,
  client_id: &str,
) -> Result<String, Error> {
  let body = json!({ "client_id": client_id });
  let raw = post(http, &format!("{base}/auth/connection"), body).await?;
  decode(raw, decode_token)
}

/// The socket's URL from a `connection` answer: the Centrifugo one.
pub fn decode_connection(json: &str) -> Result<String, String> {
  #[derive(Deserialize)]
  struct Answer {
    data: Data,
  }
  #[derive(Deserialize)]
  struct Data {
    connections: Vec<Connection>,
  }
  #[derive(Deserialize)]
  struct Connection {
    provider: String,
    credentials: Option<Credentials>,
  }
  #[derive(Deserialize)]
  struct Credentials {
    url: String,
  }
  let answer: Answer =
    ::serde_json::from_str(json).map_err(|e| e.to_string())?;
  answer
    .data
    .connections
    .into_iter()
    .filter(|connection| connection.provider == "centrifugo")
    .find_map(|connection| connection.credentials)
    .map(|credentials| credentials.url)
    .ok_or_else(|| "no centrifugo connection offered".into())
}

/// The token from an `auth/connection` answer.
pub fn decode_token(json: &str) -> Result<String, String> {
  #[derive(Deserialize)]
  struct Answer {
    data: Data,
  }
  #[derive(Deserialize)]
  struct Data {
    token: String,
  }
  let answer: Answer =
    ::serde_json::from_str(json).map_err(|e| e.to_string())?;
  Ok(answer.data.token)
}

/// A new client id: a random (version 4) uuid.
pub fn client_id() -> String {
  ::common::random::uuid()
}

async fn post(
  http: &::reqwest::Client,
  url: &str,
  body: ::serde_json::Value,
) -> Result<String, Error> {
  let response = http
    .post(url)
    .json(&body)
    .send()
    .await
    .and_then(|response| response.error_for_status())
    .map_err(Error::Http)?;
  response.text().await.map_err(Error::Http)
}

fn decode(
  raw: String,
  decode: fn(&str) -> Result<String, String>,
) -> Result<String, Error> {
  decode(&raw).map_err(|error| Error::Decode { raw, error })
}
