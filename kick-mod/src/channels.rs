//! The channels kick-mod knows: `data/kick-mod/channels.json`, an object
//! per channel by slug, written by kick-mod and editable by hand.
//!
//! ```json
//! {
//!   "<slug>": { "broadcaster_user_id": 456, "chatroom_id": 123 }
//! }
//! ```
//!
//! It's written whole, pretty-printed and sorted by slug. [`update`]
//! re-reads it just before writing, so hand edits made while kick-mod runs
//! survive, and fields kick-mod doesn't know are kept. It writes to a
//! temporary file beside it and renames that over it, so a reader sees the
//! old file or the new one, never half of one. A hand edit saved between
//! the read and the rename is still lost.

use {
  ::serde::{Deserialize, Serialize},
  ::serde_json::{Map, Value},
  ::std::{collections::BTreeMap, fs, io, path::Path, thread, time::Duration},
};

pub const DEFAULT_PATH: &str = "data/kick-mod/channels.json";

/// Tries at renaming over the file: on Windows it fails while another
/// process has the file open.
const RENAME_TRIES: u32 = 10;
const RENAME_WAIT: Duration = Duration::from_millis(100);

/// By slug.
pub type Channels = BTreeMap<String, Channel>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
  pub broadcaster_user_id: u64,
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
  match fs::read_to_string(path) {
    Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Channels::new()),
    Err(e) => Err(format!("{}: {e}", path.display())),
  }
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

/// Records what a lookup found of `slug`. A chatroom id that wasn't found
/// leaves the one there, perhaps entered by hand; other fields are kept.
pub fn record(
  channels: &mut Channels,
  slug: &str,
  broadcaster_user_id: u64,
  chatroom_id: Option<u64>,
) {
  let channel = channels.entry(slug.into()).or_insert(Channel {
    broadcaster_user_id,
    chatroom_id: None,
    rest: Map::new(),
  });
  channel.broadcaster_user_id = broadcaster_user_id;
  if chatroom_id.is_some() {
    channel.chatroom_id = chatroom_id;
  }
}

fn write(path: &Path, channels: &Channels) -> io::Result<()> {
  if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
    fs::create_dir_all(dir)?;
  }
  let mut text = ::serde_json::to_string_pretty(channels)?;
  text.push('\n');
  let mut temporary = path.as_os_str().to_owned();
  temporary.push(".tmp");
  fs::write(&temporary, text)?;
  let mut tries = 1;
  loop {
    match fs::rename(&temporary, path) {
      Ok(()) => return Ok(()),
      Err(_) if tries < RENAME_TRIES => {
        tries += 1;
        thread::sleep(RENAME_WAIT);
      }
      Err(e) => {
        let _ = fs::remove_file(&temporary);
        return Err(e);
      }
    }
  }
}
