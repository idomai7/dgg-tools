//! Kick's events: what it publishes to a channel, decoded from a
//! [`Publication`](super::centrifugo::push::Publication)'s `data`,
//! `{event, data}`, where `data` is the event's JSON as a string.
//!
//! Typed from what Kick was seen sending (2026-11-19), with Kick's names:
//! events without their `App\Events\` prefix, fields as in its JSON (in
//! snake_case where Kick's aren't). Anything else is [`Event::Other`] until
//! there's a source to type it from.
//!
//! Only the fields an object needs to mean anything are required; the rest
//! are optional, so a change degrades one field instead of the event.
//! Fields a type doesn't name are kept in its `rest`. Timestamps are unix
//! milliseconds, as in `dgg-api`. Decoding never loses
//! an event: one that doesn't match its type is [`Event::Unreadable`], with
//! the error. The frame itself, raw, is the caller's to keep.

use {
  crate::{
    rest::{self, Rest},
    time,
  },
  ::serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, Error as _},
  },
  ::serde_json::{Map, Value},
};

/// An event Kick published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
  /// A chat message, or a reply.
  ChatMessageEvent(Message),
  MessageDeletedEvent(MessageDeleted),
  /// A ban, or a timeout.
  UserBannedEvent(UserBanned),
  /// A ban or a timeout lifted.
  UserUnbannedEvent(UserUnbanned),
  PinnedMessageCreatedEvent(PinnedMessageCreated),
  PinnedMessageDeletedEvent(PinnedMessageDeleted),
  /// The chatroom's settings, all of them, after any changed. Sent with
  /// [`Event::ChatSettingsChanged`].
  ChatroomUpdatedEvent(ChatroomUpdated),
  ChatroomClearEvent(ChatroomClear),
  /// The channel's chat settings, all of them, after any changed: on
  /// `channel_<id>`, not the chatroom's channel.
  ChatSettingsChanged(ChatSettingsChanged),
  /// An event this crate doesn't type (yet).
  Other {
    event: String,
    data: String,
  },
  /// A typed event whose data didn't match its type.
  Unreadable {
    event: String,
    data: String,
    error: String,
  },
}

impl Event {
  /// Decodes a publication's `data`, `{event, data}`; `None` if it isn't
  /// one. Other fields beside `event` and `data` are ignored: they're in
  /// the frame.
  pub fn from_publication(data: &Value) -> Option<Self> {
    let event = data.get("event")?.as_str()?;
    let data = data.get("data")?.as_str()?;
    Some(Self::parse(event, data))
  }

  /// Decodes event `event`'s `data`, its JSON.
  pub fn parse(event: &str, data: &str) -> Self {
    let decoded = match event {
      "App\\Events\\ChatMessageEvent" => json(data).map(Self::ChatMessageEvent),
      "App\\Events\\MessageDeletedEvent" => {
        json(data).map(Self::MessageDeletedEvent)
      }
      "App\\Events\\UserBannedEvent" => json(data).map(Self::UserBannedEvent),
      "App\\Events\\UserUnbannedEvent" => {
        json(data).map(Self::UserUnbannedEvent)
      }
      "App\\Events\\PinnedMessageCreatedEvent" => {
        json(data).map(Self::PinnedMessageCreatedEvent)
      }
      "App\\Events\\PinnedMessageDeletedEvent" => {
        json(data).map(Self::PinnedMessageDeletedEvent)
      }
      "App\\Events\\ChatroomUpdatedEvent" => {
        json(data).map(Self::ChatroomUpdatedEvent)
      }
      "App\\Events\\ChatroomClearEvent" => {
        json(data).map(Self::ChatroomClearEvent)
      }
      "ChatSettingsChanged" => json(data).map(Self::ChatSettingsChanged),
      _ => {
        return Self::Other {
          event: event.into(),
          data: data.into(),
        };
      }
    };
    decoded.unwrap_or_else(|error| Self::Unreadable {
      event: event.into(),
      data: data.into(),
      error: error.to_string(),
    })
  }

  /// The event's name, as Kick sent it (`App\Events\ChatMessageEvent`).
  pub fn name(&self) -> &str {
    match self {
      Self::ChatMessageEvent(_) => "App\\Events\\ChatMessageEvent",
      Self::MessageDeletedEvent(_) => "App\\Events\\MessageDeletedEvent",
      Self::UserBannedEvent(_) => "App\\Events\\UserBannedEvent",
      Self::UserUnbannedEvent(_) => "App\\Events\\UserUnbannedEvent",
      Self::PinnedMessageCreatedEvent(_) => {
        "App\\Events\\PinnedMessageCreatedEvent"
      }
      Self::PinnedMessageDeletedEvent(_) => {
        "App\\Events\\PinnedMessageDeletedEvent"
      }
      Self::ChatroomUpdatedEvent(_) => "App\\Events\\ChatroomUpdatedEvent",
      Self::ChatroomClearEvent(_) => "App\\Events\\ChatroomClearEvent",
      Self::ChatSettingsChanged(_) => "ChatSettingsChanged",
      Self::Other { event, .. } | Self::Unreadable { event, .. } => event,
    }
  }

  /// Whether the event is typed (not `Other` or `Unreadable`) and nothing
  /// in it was left over: every `rest` is `None`. Absent optional fields
  /// don't count against it.
  pub fn complete(&self) -> bool {
    match self {
      Self::ChatMessageEvent(e) => e.complete(),
      Self::MessageDeletedEvent(e) => e.complete(),
      Self::UserBannedEvent(e) => e.complete(),
      Self::UserUnbannedEvent(e) => e.complete(),
      Self::PinnedMessageCreatedEvent(e) => e.complete(),
      Self::PinnedMessageDeletedEvent(e) => e.complete(),
      Self::ChatroomUpdatedEvent(e) => e.complete(),
      Self::ChatroomClearEvent(e) => e.complete(),
      Self::ChatSettingsChanged(e) => e.complete(),
      Self::Other { .. } | Self::Unreadable { .. } => false,
    }
  }
}

fn json<T: DeserializeOwned>(data: &str) -> Result<T, ::serde_json::Error> {
  ::serde_json::from_str(data)
}

/// Whether nothing was left over, here or in anything nested.
trait Complete {
  fn complete(&self) -> bool;
}

impl<T: Complete> Complete for Option<T> {
  fn complete(&self) -> bool {
    self.as_ref().is_none_or(Complete::complete)
  }
}

impl<T: Complete> Complete for Vec<T> {
  fn complete(&self) -> bool {
    self.iter().all(Complete::complete)
  }
}

/// A user, as events name one: the sender, the banned, the moderator.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct User {
  /// Always 0 in `banned_by`, even for the broadcaster.
  pub id: u64,
  pub username: String,
  pub slug: Option<String>,
  /// How chat shows them; only on senders and `pinnedBy`.
  pub identity: Option<Identity>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for User {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.identity.complete()
  }
}

/// How chat shows a user.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Identity {
  /// Such as `#00FF00`.
  pub color: Option<String>,
  #[serde(default)]
  pub badges: Vec<Badge>,
  pub badges_v2: Option<Vec<BadgeV2>>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Identity {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.badges.complete() && self.badges_v2.complete()
  }
}

/// A badge by a user's name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Badge {
  /// Such as `broadcaster`, `moderator`, `subscriber`, `verified`,
  /// `founder`. A string: Kick adds badges freely.
  pub r#type: String,
  pub text: Option<String>,
  pub active: Option<bool>,
  pub sort_order: Option<i64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Badge {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// A badge in Kick's newer form, with its image.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BadgeV2 {
  /// Such as `level`.
  pub name: String,
  /// Such as `global`.
  pub badge_type: Option<String>,
  pub image_url: Option<String>,
  /// Per badge, such as `{"level": 1}`.
  pub metadata: Option<Value>,
  pub selected: Option<bool>,
  pub sort_order: Option<i64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for BadgeV2 {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// A chat message: `ChatMessageEvent`'s data, and a pinned message.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(remote = "Self")]
pub struct Message {
  /// A uuid.
  pub id: String,
  pub chatroom_id: Option<u64>,
  /// Emotes inline as `[emote:<id>:<name>]`.
  pub content: String,
  /// `message` or `reply`.
  pub r#type: Option<String>,
  /// Set by the server, in whole seconds; unix ms. A string that isn't
  /// RFC 3339 stays in `rest`.
  #[serde(skip)]
  pub created_at: Option<i64>,
  pub sender: User,
  /// `null` when the sender sent no `message_ref`.
  pub metadata: Option<Metadata>,
  /// Only on pinned messages, where it's empty for a message that isn't a
  /// reply.
  pub thread_parent_id: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl<'de> Deserialize<'de> for Message {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    let mut decoded = Self::deserialize(deserializer)?;
    decoded.created_at = time::take(&mut decoded.rest, "created_at");
    Ok(decoded)
  }
}

impl Complete for Message {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.sender.complete() && self.metadata.complete()
  }
}

/// A message's metadata.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Metadata {
  /// Whatever the sender's client sent, echoed: an opaque string of up to
  /// 36 characters. Kick's web client sends its clock, but nothing makes
  /// it one, so don't read it as a time.
  pub message_ref: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Metadata {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// A message was deleted. Nothing says by whom.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MessageDeleted {
  /// The deletion's own uuid.
  pub id: Option<String>,
  pub message: Deleted,
  /// Whether Kick's AI moderation deleted it.
  #[serde(rename = "aiModerated")]
  pub ai_moderated: Option<bool>,
  /// The rules the AI found broken, such as `hate`.
  #[serde(rename = "violatedRules")]
  pub violated_rules: Option<Vec<String>>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for MessageDeleted {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.message.complete()
  }
}

/// The deleted message.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Deleted {
  /// The message's uuid.
  pub id: String,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Deleted {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// A user was banned (`permanent`) or timed out.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(remote = "Self")]
pub struct UserBanned {
  /// The ban's own uuid.
  pub id: Option<String>,
  pub user: User,
  /// Its `id` is always 0.
  pub banned_by: Option<User>,
  /// A ban if so, a timeout if not.
  pub permanent: bool,
  /// A timeout's, in minutes.
  pub duration: Option<u64>,
  /// When a timeout ends, in unix ms. A string that isn't RFC 3339 stays
  /// in `rest`.
  #[serde(skip)]
  pub expires_at: Option<i64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl<'de> Deserialize<'de> for UserBanned {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    let mut decoded = Self::deserialize(deserializer)?;
    decoded.expires_at = time::take(&mut decoded.rest, "expires_at");
    Ok(decoded)
  }
}

impl Complete for UserBanned {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.user.complete() && self.banned_by.complete()
  }
}

/// A ban or timeout was lifted.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct UserUnbanned {
  /// The unban's own uuid.
  pub id: Option<String>,
  pub user: User,
  pub unbanned_by: Option<User>,
  /// Whether it lifted a ban rather than a timeout.
  pub permanent: Option<bool>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for UserUnbanned {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.user.complete() && self.unbanned_by.complete()
  }
}

/// A message was pinned.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PinnedMessageCreated {
  pub message: Message,
  /// How long it stays pinned, in seconds, as a string (`"1200"`).
  pub duration: Option<String>,
  #[serde(rename = "pinnedBy")]
  pub pinned_by: Option<User>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for PinnedMessageCreated {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.message.complete() && self.pinned_by.complete()
  }
}

/// The pinned message was unpinned. Kick sends `[]`: nothing says which
/// message or by whom. Should it send an object, its fields are in `rest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedMessageDeleted {
  pub rest: Rest,
}

impl<'de> Deserialize<'de> for PinnedMessageDeleted {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
      Array(Vec<Value>),
      Object(Map<String, Value>),
    }
    match Wire::deserialize(deserializer)? {
      Wire::Array(array) if array.is_empty() => Ok(Self { rest: None }),
      Wire::Array(_) => Err(D::Error::custom("a non-empty array")),
      Wire::Object(object) => Ok(Self {
        rest: (!object.is_empty()).then_some(object),
      }),
    }
  }
}

impl Complete for PinnedMessageDeleted {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// The chatroom's settings, every one, after any changed. Nothing says who
/// changed them, or which.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatroomUpdated {
  /// The chatroom's id.
  pub id: u64,
  pub slow_mode: Option<SlowMode>,
  pub followers_mode: Option<MinDuration>,
  pub account_age: Option<MinDuration>,
  pub subscribers_mode: Option<Mode>,
  pub emotes_mode: Option<Mode>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for ChatroomUpdated {
  fn complete(&self) -> bool {
    self.rest.is_none()
      && self.slow_mode.complete()
      && self.followers_mode.complete()
      && self.account_age.complete()
      && self.subscribers_mode.complete()
      && self.emotes_mode.complete()
  }
}

/// A setting that's on or off.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Mode {
  pub enabled: bool,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Mode {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// `ChatroomUpdatedEvent`'s slow mode.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SlowMode {
  pub enabled: bool,
  /// In seconds.
  pub message_interval: Option<u64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for SlowMode {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// `ChatroomUpdatedEvent`'s followers mode and account age.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MinDuration {
  pub enabled: bool,
  /// In minutes.
  pub min_duration: Option<u64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for MinDuration {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// A chatroom was cleared. Nothing says by whom.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatroomClear {
  /// The clearing's own uuid.
  pub id: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for ChatroomClear {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// The channel's chat settings, every one, after any changed: more than
/// `ChatroomUpdatedEvent` has (`rules`, `allow_links`), with durations in
/// seconds. Nothing says who changed them, or which.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(remote = "Self")]
pub struct ChatSettingsChanged {
  /// The change's own uuid.
  pub id: Option<String>,
  pub channel: Option<Channel>,
  pub settings: Settings,
  /// Unix ms. A string that isn't RFC 3339 stays in `rest`.
  #[serde(skip)]
  pub created_at: Option<i64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl<'de> Deserialize<'de> for ChatSettingsChanged {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    let mut decoded = Self::deserialize(deserializer)?;
    decoded.created_at = time::take(&mut decoded.rest, "created_at");
    Ok(decoded)
  }
}

impl Complete for ChatSettingsChanged {
  fn complete(&self) -> bool {
    self.rest.is_none() && self.channel.complete() && self.settings.complete()
  }
}

/// A channel, as `ChatSettingsChanged` names it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Channel {
  /// The channel's id, not the chatroom's.
  pub id: u64,
  pub slug: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Channel {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}

/// A channel's chat settings.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Settings {
  pub channel_id: Option<u64>,
  pub rules: Option<String>,
  pub allow_links: Option<Mode>,
  pub emotes_only_mode: Option<Mode>,
  pub followers_only_mode: Option<Duration>,
  pub minimum_account_age: Option<Duration>,
  pub slow_mode: Option<Duration>,
  pub subscribers_only_mode: Option<Mode>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Settings {
  fn complete(&self) -> bool {
    self.rest.is_none()
      && self.allow_links.complete()
      && self.emotes_only_mode.complete()
      && self.followers_only_mode.complete()
      && self.minimum_account_age.complete()
      && self.slow_mode.complete()
      && self.subscribers_only_mode.complete()
  }
}

/// A `ChatSettingsChanged` setting with a duration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Duration {
  pub enabled: bool,
  pub duration_seconds: Option<u64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl Complete for Duration {
  fn complete(&self) -> bool {
    self.rest.is_none()
  }
}
