//! Emotes: words chat shows as pictures, from `emotes/emotes.json`.

use {
  super::{Image, images, nonempty},
  ::serde::Deserialize,
};

/// One emote.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Emote {
  /// The word that shows it, matched case-sensitively and whole.
  pub prefix: String,
  /// Who made it, when credited.
  #[serde(default, deserialize_with = "nonempty")]
  pub creator: Option<String>,
  /// Seen false for every emote.
  #[serde(default)]
  pub twitch: bool,
  /// Seen 1 for every emote; chat-gui doesn't read it.
  #[serde(default)]
  pub theme: i64,
  /// The sub tier needed to use it; 0 for anyone.
  #[serde(default)]
  pub minimum_sub_tier: u8,
  /// For Patreon members only, whatever their tier. Not seen in the file,
  /// but chat-gui reads it.
  #[serde(default)]
  pub patreon_only: bool,
  #[serde(default, deserialize_with = "images")]
  pub image: Vec<Image>,
}

/// Roles that may use every emote, as chat-gui has them.
pub const PRIVILEGED_ROLES: [&str; 3] = ["ADMIN", "MODERATOR", "PROTECTED"];

impl Emote {
  /// Whether a user of `sub_tier` (0 if not subscribed) can use it, as
  /// chat-gui decides. A user with one of the [`PRIVILEGED_ROLES`] can use
  /// any.
  pub fn usable(&self, sub_tier: u8, patreon_member: bool) -> bool {
    match self.patreon_only {
      true => patreon_member,
      false => self.minimum_sub_tier <= sub_tier,
    }
  }

  /// Its picture, if it has one.
  pub fn image(&self) -> Option<&Image> {
    self.image.first()
  }
}

/// Decodes `emotes.json`.
pub fn parse(json: &str) -> Result<Vec<Emote>, ::serde_json::Error> {
  ::serde_json::from_str(json)
}
