//! The events dgg chat sends, decoded from their frames (`EVENT payload`,
//! the payload JSON).
//!
//! Two sources:
//!
//! - Seen: what dgg was seen sending (2026-11-09), with every field it had.
//! - From chat-gui: events that only follow sending, or are rare, typed
//!   from how destiny.gg's own client (github.com/destinygg/chat-gui,
//!   2026-11-08) reads them. Only the fields it reads are required, so a
//!   frame with more decodes; their types say so.
//!
//! Anything else is [`Event::Other`] until there's a source to type it
//! from. Roles and features stay strings: dgg adds new ones freely. Like
//! chat-gui, a [`User`] needs only its nick.
//!
//! Decoding never loses a frame: one that doesn't match its type is
//! [`Event::Unreadable`], with the error. The frame itself, raw, is the
//! caller's to keep (the client's `Frame` does).

use ::serde::{Deserialize, de::DeserializeOwned};

/// A chat user, as every event describes one. Seen with every field but
/// `System`'s; only the nick is required.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
  /// The account id; -1 for the `System` user broadcasts come from.
  pub id: Option<i64>,
  pub nick: String,
  /// Such as `MODERATOR`, `PROTECTED`, `BOT`.
  #[serde(default)]
  pub roles: Vec<String>,
  /// Flairs (`flair13`) and badges (`subscriber`, `moderator`, `bot`).
  #[serde(default)]
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

/// `ERR`: a command (or the connection) was refused. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
  pub description: ErrorCode,
  /// For [`ErrorCode::Muted`]: seconds until the mute ends.
  pub mute_time_left: Option<u64>,
  /// For [`ErrorCode::SubMode`]: the tier subscriber-only mode needs.
  pub tier: Option<u8>,
  /// For [`ErrorCode::BannedPhrase`]: the phrase that blocked the message.
  pub filtered: Option<String>,
}

/// Why an [`Error`]: the codes chat-gui knows from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub enum ErrorCode {
  /// An internal problem.
  Unknown,
  NoPermission,
  /// The command was badly formatted.
  ProtocolError,
  NeedLogin,
  MsgEmpty,
  /// Over 512 characters.
  MsgTooLong,
  InvalidMsg,
  /// Sending too fast.
  Throttled,
  /// The same as the last message sent.
  Duplicate,
  /// Subscriber-only mode is on.
  SubMode,
  NeedBanReason,
  PrivMsgBanned,
  /// Over 5 connections at once; chat-gui stops reconnecting.
  TooManyConnections,
  PrivMsgAccountTooYoung,
  /// No such user.
  NotFound,
  ActivePoll,
  NoActivePoll,
  AlreadyVoted,
  /// No username yet, or chatting was disabled.
  NoChatting,
  DuplicatePhrase,
  NoPhrase,
  InvalidPhrase,
  CantBanProtected,
  Muted,
  /// Banned; chat-gui stops reconnecting.
  Banned,
  BannedPhrase,
  Other(String),
}

impl From<String> for ErrorCode {
  fn from(code: String) -> Self {
    match code.as_str() {
      "unknown" => Self::Unknown,
      "nopermission" => Self::NoPermission,
      "protocolerror" => Self::ProtocolError,
      "needlogin" => Self::NeedLogin,
      "msgempty" => Self::MsgEmpty,
      "msgtoolong" => Self::MsgTooLong,
      "invalidmsg" => Self::InvalidMsg,
      "throttled" => Self::Throttled,
      "duplicate" => Self::Duplicate,
      "submode" => Self::SubMode,
      "needbanreason" => Self::NeedBanReason,
      "privmsgbanned" => Self::PrivMsgBanned,
      "toomanyconnections" => Self::TooManyConnections,
      "privmsgaccounttooyoung" => Self::PrivMsgAccountTooYoung,
      "notfound" => Self::NotFound,
      "activepoll" => Self::ActivePoll,
      "noactivepoll" => Self::NoActivePoll,
      "alreadyvoted" => Self::AlreadyVoted,
      "nochatting" => Self::NoChatting,
      "duplicatephrase" => Self::DuplicatePhrase,
      "nophrase" => Self::NoPhrase,
      "invalidphrase" => Self::InvalidPhrase,
      "cantbanprotected" => Self::CantBanProtected,
      "muted" => Self::Muted,
      "banned" => Self::Banned,
      "bannedphrase" => Self::BannedPhrase,
      _ => Self::Other(code),
    }
  }
}

impl ErrorCode {
  /// Whether it refused a chat message just sent (which chat never echoes
  /// back), as chat-gui counts them.
  pub fn rejects_message(&self) -> bool {
    matches!(
      self,
      Self::NeedLogin
        | Self::NoChatting
        | Self::MsgEmpty
        | Self::MsgTooLong
        | Self::InvalidMsg
        | Self::Muted
        | Self::SubMode
        | Self::Throttled
        | Self::BannedPhrase
        | Self::Duplicate
    )
  }
}

/// `PRIVMSG`: a whisper to this connection's user. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PrivMsg {
  pub nick: String,
  pub data: String,
  /// Unix ms.
  pub timestamp: i64,
  /// For marking it read through dgg's API.
  #[serde(rename = "messageid")]
  pub message_id: Option<i64>,
}

/// `BAN`, `UNBAN` or `UNMUTE`: `nick` did it to `target`. From chat-gui,
/// which reads no duration. An `UNMUTE` by `System` is the website lifting
/// a mute (a subscription or donation).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Moderation {
  pub nick: String,
  #[serde(rename = "data")]
  pub target: String,
  /// Unix ms.
  pub timestamp: i64,
}

/// `SUBONLY`: subscriber-only mode changed. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SubOnly {
  /// `on` or `off`.
  #[serde(rename = "data")]
  pub state: String,
  /// The tier it needs, when on; 1 if left out.
  pub tier: Option<u8>,
  /// Who changed it.
  pub nick: Option<String>,
  /// Unix ms.
  pub timestamp: i64,
}

impl SubOnly {
  /// The tier subscribers need to chat, or `None` if it's off.
  pub fn tier(&self) -> Option<u8> {
    (self.state == "on").then(|| self.tier.unwrap_or(1))
  }
}

/// `PIN`: `nick` pinned a message, or unpinned (`data` empty). From
/// chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Pin {
  pub nick: String,
  pub data: String,
  pub uuid: String,
  /// Unix ms.
  pub timestamp: i64,
}

/// `ADDPHRASE` or `REMOVEPHRASE`: a banned phrase. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Phrase {
  #[serde(rename = "data")]
  pub phrase: String,
  /// Unix ms.
  pub timestamp: i64,
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
  Err(Error),
  PrivMsg(PrivMsg),
  /// The whisper just sent was delivered. From chat-gui.
  PrivMsgSent,
  Ban(Moderation),
  Unban(Moderation),
  Unmute(Moderation),
  SubOnly(SubOnly),
  Pin(Pin),
  AddPhrase(Phrase),
  RemovePhrase(Phrase),
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
      "ERR" => json(payload).map(Self::Err),
      "PRIVMSG" => json(payload).map(Self::PrivMsg),
      // chat-gui reads nothing from it.
      "PRIVMSGSENT" => Ok(Self::PrivMsgSent),
      "BAN" => json(payload).map(Self::Ban),
      "UNBAN" => json(payload).map(Self::Unban),
      "UNMUTE" => json(payload).map(Self::Unmute),
      "SUBONLY" => json(payload).map(Self::SubOnly),
      "PIN" => json(payload).map(Self::Pin),
      "ADDPHRASE" => json(payload).map(Self::AddPhrase),
      "REMOVEPHRASE" => json(payload).map(Self::RemovePhrase),
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
      Self::Err(_) => "ERR",
      Self::PrivMsg(_) => "PRIVMSG",
      Self::PrivMsgSent => "PRIVMSGSENT",
      Self::Ban(_) => "BAN",
      Self::Unban(_) => "UNBAN",
      Self::Unmute(_) => "UNMUTE",
      Self::SubOnly(_) => "SUBONLY",
      Self::Pin(_) => "PIN",
      Self::AddPhrase(_) => "ADDPHRASE",
      Self::RemovePhrase(_) => "REMOVEPHRASE",
      Self::Other { event, .. } | Self::Unreadable { event, .. } => event,
    }
  }
}
