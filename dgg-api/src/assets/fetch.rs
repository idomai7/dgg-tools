//! Fetching the assets from the CDN (feature `http`).
//!
//! Each fetch keeps the file as served, alongside its decoding, so a tool
//! can store the exact bytes; one that doesn't decode is kept too, in the
//! error. The `reqwest` client is the caller's: its user agent, timeouts,
//! and (with rustls) crypto provider.

use {
  super::{
    Emote, Flair, emote, emotes_css_url, emotes_url, flair, flairs_css_url,
    flairs_url,
  },
  ::std::{error, fmt},
};

/// A fetched file: as served, and decoded.
#[derive(Clone, Debug)]
pub struct Fetched<T> {
  pub raw: String,
  pub value: T,
}

/// Why a fetch failed.
#[derive(Debug)]
pub enum Error {
  /// No answer, or not a success.
  Http(::reqwest::Error),
  /// An answer that didn't decode, kept as served.
  Decode {
    raw: String,
    error: ::serde_json::Error,
  },
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::Http(e) => write!(f, "fetch failed: {e}"),
      Self::Decode { error, .. } => write!(f, "didn't decode: {error}"),
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::Http(e) => Some(e),
      Self::Decode { error, .. } => Some(error),
    }
  }
}

/// Fetches the emotes from `cdn` (see [`emotes_url`] for `cache_key`).
pub async fn emotes(
  client: &::reqwest::Client,
  cdn: &str,
  cache_key: Option<&str>,
) -> Result<Fetched<Vec<Emote>>, Error> {
  fetch(client, &emotes_url(cdn, cache_key), emote::parse).await
}

/// Fetches the flairs from `cdn` (see [`emotes_url`] for `cache_key`).
pub async fn flairs(
  client: &::reqwest::Client,
  cdn: &str,
  cache_key: Option<&str>,
) -> Result<Fetched<Vec<Flair>>, Error> {
  fetch(client, &flairs_url(cdn, cache_key), flair::parse).await
}

/// Fetches the emotes' stylesheet, as served.
pub async fn emotes_css(
  client: &::reqwest::Client,
  cdn: &str,
  cache_key: Option<&str>,
) -> Result<String, Error> {
  text(client, &emotes_css_url(cdn, cache_key)).await
}

/// Fetches the flairs' stylesheet, as served.
pub async fn flairs_css(
  client: &::reqwest::Client,
  cdn: &str,
  cache_key: Option<&str>,
) -> Result<String, Error> {
  text(client, &flairs_css_url(cdn, cache_key)).await
}

async fn fetch<T>(
  client: &::reqwest::Client,
  url: &str,
  parse: fn(&str) -> Result<T, ::serde_json::Error>,
) -> Result<Fetched<T>, Error> {
  let raw = text(client, url).await?;
  match parse(&raw) {
    Ok(value) => Ok(Fetched { raw, value }),
    Err(error) => Err(Error::Decode { raw, error }),
  }
}

async fn text(client: &::reqwest::Client, url: &str) -> Result<String, Error> {
  let response = client
    .get(url)
    .send()
    .await
    .and_then(|response| response.error_for_status())
    .map_err(Error::Http)?;
  response.text().await.map_err(Error::Http)
}
