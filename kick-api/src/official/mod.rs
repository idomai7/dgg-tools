//! Kick's official public API.
//!
//! - [`oauth`]: signing in, and the app's and users' tokens.
//!
//! The `reqwest` client is the caller's: its user agent, timeouts, and
//! (with rustls) crypto provider.

pub mod oauth;

use ::std::{error, fmt};

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
      Self::Status { .. } | Self::Decode { .. } => None,
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
