//! The events dgg chat sends, decoded from their frames (`EVENT payload`,
//! the payload JSON).
//!
//! Typed from what dgg was seen sending (2026-11-09): every event below,
//! with every field it had. Events not seen yet, or seen only empty (`ME`
//! while logged out, `SPOTLIGHTS`), are [`Event::Other`] until there's a
//! sample to type them from. Roles and features stay strings: dgg adds new
//! ones freely.
//!
//! Decoding never loses a frame: one that doesn't match its type is
//! [`Event::Unreadable`], with the error. The frame itself, raw, is the
//! caller's to keep (the client's `Frame` does).

use ::serde::{Deserialize, de::DeserializeOwned};

/// A chat user, as every event describes one.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
  /// The account id; -1 for the `System` user broadcasts come from.
  pub id: i64,
  pub nick: String,
  /// Such as `MODERATOR`, `PROTECTED`, `BOT`.
  pub roles: Vec<String>,
  /// Flairs (`flair13`) and badges (`subscriber`, `moderator`, `bot`).
  pub features: Vec<String>,
  /// When the account was made, as dgg writes it (`2020-01-01T00:00:00Z`);
  /// `None` for `System`.
  pub created_date: Option<String>,
  /// The embed they're watching, if any.
  pub watching: Option<Watching>,
  pub subscription: Option<Subscription>,
  /// `false` where dgg leaves it out (`System`).
  #[serde(default)]
  pub patreon_member: bool,
  /// Their avatar's name (`buddy`, `temmie`, ...); `None` for `System`.
  pub photo: Option<String>,
}

/// An embed: `platform` (`kick`, `youtube`, `twitch`, `kick-vod`,
/// `angelthump`, ...) and its id there.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Watching {
  pub platform: String,
  pub id: String,
}

/// A user's subscription.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Subscription {
  /// 1 to 5.
  pub tier: u8,
  /// `destiny.gg` or `Patreon`.
  pub source: String,
}

/// `MSG`: a chat message.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Message {
  #[serde(flatten)]
  pub user: User,
  /// Unix ms.
  pub timestamp: i64,
  pub data: String,
}

/// `JOIN` or `QUIT`: a user's first connection opened, or last closed.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Presence {
  #[serde(flatten)]
  pub user: User,
  /// Unix ms.
  pub timestamp: i64,
}

/// `NAMES`: everyone connected, sent on connecting.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Names {
  pub users: Vec<User>,
  /// Connections, which can be more than users.
  #[serde(rename = "connectioncount")]
  pub connection_count: u64,
}

/// `MUTE`: `by` muted `target`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Mute {
  #[serde(flatten)]
  pub by: User,
  /// Unix ms.
  pub timestamp: i64,
  /// Seconds.
  pub duration: u64,
  /// The muted user's nick.
  #[serde(rename = "data")]
  pub target: String,
}

/// `DEATH`: a user muted by chat's game, with the reason it gives.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Death {
  #[serde(flatten)]
  pub user: User,
  /// Unix ms.
  pub timestamp: i64,
  /// Seconds.
  pub duration: u64,
  #[serde(rename = "data")]
  pub reason: String,
}

/// `SUBSCRIPTION`: a user subscribed, shown in chat until
/// `expiration_timestamp`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscribed {
  /// Unix ms.
  pub timestamp: i64,
  pub nick: String,
  pub user: User,
  /// Their message with it; often empty.
  pub data: String,
  pub uuid: String,
  /// In cents.
  pub amount: u64,
  /// Unix ms.
  pub expiration_timestamp: i64,
  pub tier: u8,
  /// Such as `Tier I`.
  pub tier_label: String,
  /// Months in a row.
  pub streak: u64,
}

/// `BROADCAST`: a message to everyone, from `System`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Broadcast {
  /// Unix ms.
  pub timestamp: i64,
  pub nick: String,
  pub user: User,
  pub data: String,
  pub uuid: String,
}

/// `XPOST`: a post on X shown in chat until `expiration_timestamp`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XPost {
  /// Unix ms.
  pub timestamp: i64,
  /// Unix ms.
  pub expiration_timestamp: i64,
  pub handle: String,
  pub post_id: String,
  pub text: String,
  pub url: String,
  /// When it was posted, as dgg writes it (`2026-11-09T09:01:50.000Z`).
  pub created_at: String,
  pub uuid: String,
}

/// One decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
  Msg(Message),
  Join(Presence),
  Quit(Presence),
  /// A connected user changed (their flairs, what they're watching, ...).
  UpdateUser(User),
  Names(Names),
  /// Recent frames (`MSG`s, `XPOST`s), oldest first, sent on connecting.
  History(Vec<Event>),
  Mute(Mute),
  Death(Death),
  Subscription(Subscribed),
  Broadcast(Broadcast),
  XPost(XPost),
  /// Paid events still showing (`XPOST`s), sent on connecting.
  PaidEvents(Vec<Event>),
  /// An event this crate doesn't type (yet).
  Other {
    event: String,
    payload: String,
  },
  /// A typed event whose payload didn't match its type.
  Unreadable {
    event: String,
    payload: String,
    error: String,
  },
}

fn json<T: DeserializeOwned>(payload: &str) -> Result<T, ::serde_json::Error> {
  ::serde_json::from_str(payload)
}

/// Decodes each of a JSON array of frames.
fn frames(payload: &str) -> Result<Vec<Event>, ::serde_json::Error> {
  let frames: Vec<String> = json(payload)?;
  Ok(frames.iter().map(|frame| Event::parse(frame)).collect())
}

impl Event {
  /// Decodes `frame`, `EVENT payload`.
  pub fn parse(frame: &str) -> Self {
    let (event, payload) = frame.split_once(' ').unwrap_or((frame, ""));
    let decoded = match event {
      "MSG" => json(payload).map(Self::Msg),
      "JOIN" => json(payload).map(Self::Join),
      "QUIT" => json(payload).map(Self::Quit),
      "UPDATEUSER" => json(payload).map(Self::UpdateUser),
      "NAMES" => json(payload).map(Self::Names),
      "HISTORY" => frames(payload).map(Self::History),
      "MUTE" => json(payload).map(Self::Mute),
      "DEATH" => json(payload).map(Self::Death),
      "SUBSCRIPTION" => json(payload).map(Self::Subscription),
      "BROADCAST" => json(payload).map(Self::Broadcast),
      "XPOST" => json(payload).map(Self::XPost),
      "PAIDEVENTS" => frames(payload).map(Self::PaidEvents),
      _ => {
        return Self::Other {
          event: event.into(),
          payload: payload.into(),
        };
      }
    };
    decoded.unwrap_or_else(|error| Self::Unreadable {
      event: event.into(),
      payload: payload.into(),
      error: error.to_string(),
    })
  }

  /// The event's name, as in its frame (`MSG`).
  pub fn name(&self) -> &str {
    match self {
      Self::Msg(_) => "MSG",
      Self::Join(_) => "JOIN",
      Self::Quit(_) => "QUIT",
      Self::UpdateUser(_) => "UPDATEUSER",
      Self::Names(_) => "NAMES",
      Self::History(_) => "HISTORY",
      Self::Mute(_) => "MUTE",
      Self::Death(_) => "DEATH",
      Self::Subscription(_) => "SUBSCRIPTION",
      Self::Broadcast(_) => "BROADCAST",
      Self::XPost(_) => "XPOST",
      Self::PaidEvents(_) => "PAIDEVENTS",
      Self::Other { event, .. } | Self::Unreadable { event, .. } => event,
    }
  }
}
