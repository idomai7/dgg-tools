//! Flairs: the badges by a chatter's name, from `flairs/flairs.json`. A
//! chat user's `features` are the names of their flairs.

use {
  super::{Image, images, nonempty},
  ::serde::Deserialize,
};

/// One flair.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flair {
  /// What a user's `features` call it: `flair13`, `moderator`, ...
  pub name: String,
  /// What chat shows it as: `Subscriber Tier 1`, `Moderator`, ...
  #[serde(default)]
  pub label: String,
  #[serde(default, deserialize_with = "nonempty")]
  pub description: Option<String>,
  /// In a user's info card, chat-gui shows a hidden flair as its label
  /// rather than its picture. In chat it shows as any other.
  #[serde(default)]
  pub hidden: bool,
  /// Lower comes first.
  #[serde(default)]
  pub priority: i64,
  /// The colour it gives a username (`#RRGGBB`), if any.
  #[serde(default, deserialize_with = "nonempty")]
  pub color: Option<String>,
  /// Gives a username a rainbow instead (styled by `flairs.css`).
  #[serde(default)]
  pub rainbow_color: bool,
  #[serde(default, deserialize_with = "images")]
  pub image: Vec<Image>,
}

/// The flair of each sub tier, 1 to 5, as chat-gui has them.
pub const SUB_TIERS: [&str; 5] =
  ["flair13", "flair1", "flair3", "flair8", "flair42"];

/// The flair every subscriber has, whatever their tier.
pub const SUBSCRIBER: &str = "subscriber";

/// The flair of sub tier `tier`, if it has one.
pub fn sub_tier(tier: u8) -> Option<&'static str> {
  SUB_TIERS.get(usize::from(tier).checked_sub(1)?).copied()
}

impl Flair {
  /// Whether it colours a username.
  pub fn colors(&self) -> bool {
    self.rainbow_color || self.color.is_some()
  }

  /// Its picture, if it has one.
  pub fn image(&self) -> Option<&Image> {
    self.image.first()
  }
}

/// The flair that colours the username of a user with `features`, as
/// chat-gui picks it: of theirs that colour, the first by priority.
pub fn username_color<'a, Name: AsRef<str>>(
  flairs: &'a [Flair],
  features: &[Name],
) -> Option<&'a Flair> {
  flairs
    .iter()
    .filter(|flair| flair.colors())
    .filter(|flair| {
      features
        .iter()
        .any(|feature| feature.as_ref() == flair.name)
    })
    .min_by_key(|flair| flair.priority)
}

/// Decodes `flairs.json`.
pub fn parse(json: &str) -> Result<Vec<Flair>, ::serde_json::Error> {
  ::serde_json::from_str(json)
}
