//! The channels kick-mod knows: `data/kick-mod/channels.json`, an object
//! per channel by slug, written by kick-mod and editable by hand.
//!
//! ```json
//! {
//!   "<slug>": {
//!     "broadcaster_user_id": 456,
//!     "channel_id": 789,
//!     "chatroom_id": 123
//!   }
//! }
//! ```
//!
//! It's written whole, pretty-printed and sorted by slug. [`update`]
//! re-reads it just before writing, so hand edits made while kick-mod runs
//! survive, and fields kick-mod doesn't know are kept. It's
//! [replaced](::common::atomic_file) whole, never half-written. A hand edit
//! saved between the read and the write is still lost.

use {
  ::common::{atomic_file, fs::MissingOk},
  ::serde::{Deserialize, Serialize},
  ::serde_json::{Map, Value},
  ::std::{collections::BTreeMap, fs, io, path::Path},
};

pub const DEFAULT_PATH: &str = "data/kick-mod/channels.json";

/// By slug.
pub type Channels = BTreeMap<String, Channel>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
  pub broadcaster_user_id: u64,
  /// The channel's own id, which Kick's chat history is asked by: `null`
  /// until it's known, and missing from files written before it was kept.
  #[serde(default)]
  pub channel_id: Option<u64>,
  /// `null` until it's known: entered by hand when the lookup fails.
  pub chatroom_id: Option<u64>,
  /// Fields kick-mod doesn't know, kept as they were.
  #[serde(flatten)]
  pub rest: Map<String, Value>,
}

pub fn parse(text: &str) -> Result<Channels, String> {
  ::serde_json::from_str(text).map_err(|e| e.to_string())
}

/// The channels in `path`: none if there's no file yet.
pub fn load(path: &Path) -> Result<Channels, String> {
  let in_path = |e: String| format!("{}: {e}", path.display());
  let text = fs::read_to_string(path)
    .missing_ok()
    .map_err(|e| in_path(e.to_string()))?;
  text.map_or(Ok(Channels::new()), |text| parse(&text).map_err(in_path))
}

/// Re-reads `path`, applies `change`, and writes it back: the channels as
/// written.
pub fn update(
  path: &Path,
  change: impl FnOnce(&mut Channels),
) -> Result<Channels, String> {
  let mut channels = load(path)?;
  change(&mut channels);
  write(path, &channels).map_err(|e| format!("{}: {e}", path.display()))?;
  Ok(channels)
}

/// Records what a lookup found of `slug`. A channel or chatroom id that
/// wasn't found leaves the one there, perhaps entered by hand; other fields
/// are kept.
pub fn record(
  channels: &mut Channels,
  slug: &str,
  broadcaster_user_id: u64,
  channel_id: Option<u64>,
  chatroom_id: Option<u64>,
) {
  let channel = channels.entry(slug.into()).or_insert(Channel {
    broadcaster_user_id,
    channel_id: None,
    chatroom_id: None,
    rest: Map::new(),
  });
  channel.broadcaster_user_id = broadcaster_user_id;
  if channel_id.is_some() {
    channel.channel_id = channel_id;
  }
  if chatroom_id.is_some() {
    channel.chatroom_id = chatroom_id;
  }
}

fn write(path: &Path, channels: &Channels) -> io::Result<()> {
  let mut text = ::serde_json::to_string_pretty(channels)?;
  text.push('\n');
  atomic_file::replace(path, text)
}
