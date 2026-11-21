//! Kick's official public API.
//!
//! - [`oauth`]: signing in, and the app's and users' tokens.
//! - [`Api`]: the endpoints, with a token it keeps fresh.
//!
//! The `reqwest` client is the caller's: its user agent, timeouts, and
//! (with rustls) crypto provider.

pub mod api;
pub mod oauth;

pub use api::Api;

use ::std::{
  error, fmt,
  time::{SystemTime, UNIX_EPOCH},
};

/// Why a request failed.
#[derive(Debug)]
pub enum Error {
  /// No answer.
  Http(::reqwest::Error),
  /// An answer that wasn't a success, kept as served.
  Status { status: u16, body: String },
  /// A success that didn't decode, kept as served.
  Decode { raw: String, error: String },
  /// A user's token is due a refresh but has no refresh token: the user
  /// has to sign in again.
  NoRefreshToken,
}

impl Error {
  /// The HTTP status, if there was an answer that wasn't a success.
  pub fn status(&self) -> Option<u16> {
    match self {
      Self::Status { status, .. } => Some(*status),
      _ => None,
    }
  }

  /// Kick's `message`, from an answer that wasn't a success.
  pub fn message(&self) -> Option<String> {
    let Self::Status { body, .. } = self else {
      return None;
    };
    let body: ::serde_json::Value = ::serde_json::from_str(body).ok()?;
    Some(body.get("message")?.as_str()?.to_owned())
  }

  /// A 429: too many requests, to retry later.
  pub fn rate_limited(&self) -> bool {
    self.status() == Some(429)
  }

  /// A 403 that Cloudflare answered, not Kick: its page, not Kick's JSON.
  /// It has refused some clients (KickDevDocs #281, #434).
  pub fn cloudflare(&self) -> bool {
    match self {
      Self::Status { status: 403, body } => {
        ::serde_json::from_str::<::serde_json::Value>(body).is_err()
          && body.to_lowercase().contains("cloudflare")
      }
      _ => false,
    }
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::Http(e) => write!(f, "request failed: {e}"),
      Self::Status { status, body } => write!(f, "answered {status}: {body}"),
      Self::Decode { error, .. } => write!(f, "didn't decode: {error}"),
      Self::NoRefreshToken => write!(f, "no refresh token: sign in again"),
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::Http(e) => Some(e),
      Self::Status { .. } | Self::Decode { .. } | Self::NoRefreshToken => None,
    }
  }
}

/// Sends `request`, and returns the answer's body if it's a success.
async fn send(request: ::reqwest::RequestBuilder) -> Result<String, Error> {
  let response = request.send().await.map_err(Error::Http)?;
  let status = response.status();
  let body = response.text().await.map_err(Error::Http)?;
  if status.is_success() {
    Ok(body)
  } else {
    Err(Error::Status {
      status: status.as_u16(),
      body,
    })
  }
}

/// Now, in unix milliseconds.
fn now() -> i64 {
  let since = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default();
  i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
}
