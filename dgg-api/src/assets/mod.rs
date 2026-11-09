//! Chat's assets: its emotes and flairs, which destiny.gg's CDN serves as
//! JSON (`emotes/emotes.json`, `flairs/flairs.json`).
//!
//! Typed from what the CDN served (2026-11-09) and how chat-gui
//! (github.com/destinygg/chat-gui, 2026-11-08) reads them. Every field but
//! the names has a default, so a file that drops one still decodes. The
//! sprites and animations are in matching `.css` files: rendering, not
//! typed here.
//!
//! The CDN caches the files for hours. Chat announces new ones with
//! `REFRESHCHATASSETS {"cacheKey":...}`; asking with that key (see
//! [`emotes_url`]) gets them fresh.

pub mod emote;
pub mod flair;

pub use {emote::Emote, flair::Flair};

use ::serde::{Deserialize, Deserializer};

/// destiny.gg's CDN.
pub const DEFAULT_CDN: &str = "https://cdn.destiny.gg";

/// Where `cdn` serves the emotes; with a cache key, a copy that's at least
/// as new as the key.
pub fn emotes_url(cdn: &str, cache_key: Option<&str>) -> String {
  url(cdn, "emotes/emotes.json", cache_key)
}

/// Where `cdn` serves the flairs; see [`emotes_url`].
pub fn flairs_url(cdn: &str, cache_key: Option<&str>) -> String {
  url(cdn, "flairs/flairs.json", cache_key)
}

fn url(cdn: &str, path: &str, cache_key: Option<&str>) -> String {
  let cdn = cdn.trim_end_matches('/');
  match cache_key {
    // Keys are the site's version or a timestamp, so need no escaping.
    Some(key) => format!("{cdn}/{path}?_={key}"),
    None => format!("{cdn}/{path}"),
  }
}

/// An emote's or flair's picture.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Image {
  pub url: String,
  /// The file's name.
  #[serde(default)]
  pub name: String,
  /// Seen: `image/png`, `image/gif`, `image/avif`, `image/webp`.
  #[serde(default)]
  pub mime: String,
  #[serde(default)]
  pub width: u32,
  #[serde(default)]
  pub height: u32,
}

/// An `image` list. Each seen had one entry; a flair with no picture has
/// one whose every field is null, which is left out.
fn images<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<Vec<Image>, D::Error> {
  #[derive(Deserialize)]
  struct Entry {
    url: Option<String>,
    name: Option<String>,
    mime: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
  }
  let entries = Option::<Vec<Entry>>::deserialize(deserializer)?;
  Ok(
    entries
      .unwrap_or_default()
      .into_iter()
      .filter_map(|entry| {
        Some(Image {
          url: entry.url?,
          name: entry.name.unwrap_or_default(),
          mime: entry.mime.unwrap_or_default(),
          width: entry.width.unwrap_or_default(),
          height: entry.height.unwrap_or_default(),
        })
      })
      .collect(),
  )
}

/// A string where empty means none, as the files use (`""` or `null`).
fn nonempty<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<Option<String>, D::Error> {
  let text = Option::<String>::deserialize(deserializer)?;
  Ok(text.filter(|text| !text.is_empty()))
}
