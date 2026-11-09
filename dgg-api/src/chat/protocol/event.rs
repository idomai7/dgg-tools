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

use ::serde::{
  Deserialize, Deserializer,
  de::{DeserializeOwned, Error as _},
};

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

/// `GIFTSUB`: `user` gifted `recipient` a subscription, shown in chat until
/// `expiration_timestamp`. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GiftSub {
  pub user: User,
  pub recipient: User,
  pub tier: u8,
  /// Such as `Tier I`.
  pub tier_label: String,
  /// Months.
  pub amount: u64,
  /// Whether it's one of a [`MassGift`]'s.
  #[serde(default)]
  pub from_mass_gift: bool,
  /// Their message with it; often empty.
  pub data: String,
  /// Unix ms.
  pub timestamp: i64,
  /// Unix ms.
  pub expiration_timestamp: i64,
  pub uuid: String,
}

/// `MASSGIFT`: `user` gifted `quantity` subscriptions to whoever was in
/// chat. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MassGift {
  pub user: User,
  pub tier: u8,
  /// Such as `Tier I`.
  pub tier_label: String,
  /// Months each.
  pub amount: u64,
  pub quantity: u64,
  pub data: String,
  /// Unix ms.
  pub timestamp: i64,
  /// Unix ms.
  pub expiration_timestamp: i64,
  pub uuid: String,
}

/// `DONATION`: shown in chat until `expiration_timestamp`. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Donation {
  pub user: User,
  /// In cents.
  pub amount: u64,
  /// Their message with it.
  pub data: String,
  /// Unix ms.
  pub timestamp: i64,
  /// Unix ms.
  pub expiration_timestamp: i64,
  pub uuid: String,
  /// A URL of audio played with it, if any.
  pub audio: Option<String>,
}

/// `POLLSTART`: a poll, as it stands. Sent when one starts, and on
/// connecting while one runs. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PollStart {
  /// Who started it.
  pub nick: String,
  pub question: String,
  pub options: Vec<String>,
  /// Votes per option so far (zeros while hidden).
  pub totals: Vec<u64>,
  #[serde(rename = "totalvotes")]
  pub total_votes: u64,
  /// Votes count by subscription tier.
  pub weighted: bool,
  /// Results hidden until it ends.
  #[serde(default)]
  pub hidden: bool,
  /// When it started, and the server's time now, as dgg writes them.
  pub start: String,
  pub now: String,
  /// How long it runs, in ms.
  pub time: u64,
  /// Whether this connection's user can still vote.
  #[serde(rename = "canvote")]
  pub can_vote: bool,
  /// Their vote: an option's number, from 1; 0 or less for none.
  #[serde(rename = "myvote")]
  pub my_vote: i64,
}

/// `POLLSTOP`: the poll ended; a hidden one's results come with it. From
/// chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PollStop {
  pub totals: Option<Vec<u64>>,
  #[serde(rename = "totalvotes")]
  pub total_votes: Option<u64>,
}

/// `VOTECAST`: someone voted, with the weight of `quantity` votes. A hidden
/// poll's votes come without the option. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct VoteCast {
  #[serde(default, deserialize_with = "vote")]
  pub vote: Option<u64>,
  pub quantity: u64,
}

/// `VOTECOUNTED`: this connection's vote counted. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct VoteCounted {
  #[serde(default, deserialize_with = "vote")]
  pub vote: Option<u64>,
}

/// An option's number, which chat-gui handles as either a number or a
/// string of one (it's sent as a string).
fn vote<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<Option<u64>, D::Error> {
  #[derive(Deserialize)]
  #[serde(untagged)]
  enum Vote {
    Number(u64),
    Text(String),
  }
  match Option::<Vote>::deserialize(deserializer)? {
    None => Ok(None),
    Some(Vote::Number(number)) => Ok(Some(number)),
    Some(Vote::Text(text)) => text.parse().map(Some).map_err(D::Error::custom),
  }
}

/// `SPOTLIGHT`: `spotlighted_by` spotlighted `user`'s message: shown in
/// chat until `expiration_timestamp`, emphasised until
/// `retention_timestamp`. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spotlight {
  /// Identifies the message: a hash of its time, author and text.
  pub key: String,
  /// The message's author.
  pub user: User,
  /// The message's text.
  pub data: String,
  /// When the message was sent, unix ms.
  pub message_timestamp: Option<i64>,
  /// Who spotlighted it.
  pub spotlighted_by: String,
  /// Unix ms.
  pub timestamp: i64,
  /// Unix ms.
  pub expiration_timestamp: i64,
  /// Unix ms.
  pub retention_timestamp: Option<i64>,
  pub uuid: String,
}

/// `UNSPOTLIGHT`: the spotlight with this key ended. From chat-gui.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Unspotlight {
  pub key: String,
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
  GiftSub(GiftSub),
  MassGift(MassGift),
  Donation(Donation),
  PollStart(PollStart),
  PollStop(PollStop),
  VoteCast(VoteCast),
  VoteCounted(VoteCounted),
  Spotlight(Spotlight),
  Unspotlight(Unspotlight),
  /// Spotlights still showing, sent on connecting (seen, empty).
  Spotlights(Vec<Spotlight>),
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
      "GIFTSUB" => json(payload).map(Self::GiftSub),
      "MASSGIFT" => json(payload).map(Self::MassGift),
      "DONATION" => json(payload).map(Self::Donation),
      "POLLSTART" => json(payload).map(Self::PollStart),
      "POLLSTOP" => json(payload).map(Self::PollStop),
      "VOTECAST" => json(payload).map(Self::VoteCast),
      "VOTECOUNTED" => json(payload).map(Self::VoteCounted),
      "SPOTLIGHT" => json(payload).map(Self::Spotlight),
      "UNSPOTLIGHT" => json(payload).map(Self::Unspotlight),
      "SPOTLIGHTS" => json::<Option<Vec<Spotlight>>>(payload)
        .map(|list| Self::Spotlights(list.unwrap_or_default())),
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
      Self::GiftSub(_) => "GIFTSUB",
      Self::MassGift(_) => "MASSGIFT",
      Self::Donation(_) => "DONATION",
      Self::PollStart(_) => "POLLSTART",
      Self::PollStop(_) => "POLLSTOP",
      Self::VoteCast(_) => "VOTECAST",
      Self::VoteCounted(_) => "VOTECOUNTED",
      Self::Spotlight(_) => "SPOTLIGHT",
      Self::Unspotlight(_) => "UNSPOTLIGHT",
      Self::Spotlights(_) => "SPOTLIGHTS",
      Self::Other { event, .. } | Self::Unreadable { event, .. } => event,
    }
  }
}
