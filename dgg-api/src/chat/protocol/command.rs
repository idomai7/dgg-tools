//! What a client sends chat, encoded as frames (`EVENT {json}`).
//!
//! Taken from destiny.gg's own client (github.com/destinygg/chat-gui,
//! 2026-11-08): each command is sent as it sends it. Most need a logged-in
//! connection, and the moderation ones the right role. Chat doesn't answer
//! a command directly: it takes effect as the event everyone gets (`MUTE`,
//! `PIN`, ...), or fails as an `ERR` to the sender. Only a whisper is
//! acknowledged, with `PRIVMSGSENT`.

use {
  ::serde_json::{Value, json},
  ::std::time::Duration,
};

/// How long a ban lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanLength {
  Permanent,
  For(Duration),
}

/// Subscriber-only mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubOnly {
  Off,
  /// On, for subscribers of this tier (1 to 5) and up.
  Tier(u8),
}

/// A poll to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poll {
  pub question: String,
  pub options: Vec<String>,
  /// How long it runs. dgg's client keeps this within its own limits; the
  /// server may too.
  pub time: Duration,
  /// Votes count by subscription tier.
  pub weighted: bool,
  /// The results are hidden until it ends.
  pub hidden: bool,
}

/// A command for chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
  /// A chat message.
  Msg(String),
  /// A whisper to `nick`.
  PrivMsg {
    nick: String,
    data: String,
  },
  /// A vote in the running poll: an option's number, from 1.
  CastVote(String),
  /// Mutes `nick`, for `duration` or the server's default.
  Mute {
    nick: String,
    duration: Option<Duration>,
  },
  Unmute(String),
  /// Bans `nick`, with a reason (required), and their IP too if `ip`.
  Ban {
    nick: String,
    reason: String,
    length: BanLength,
    ip: bool,
  },
  Unban(String),
  SubOnly(SubOnly),
  /// A message to everyone, from `System`.
  Broadcast(String),
  /// Pins a message above chat.
  Pin(String),
  Unpin,
  /// Bans a phrase (a regex, if it parses as one).
  AddPhrase(String),
  RemovePhrase(String),
  StartPoll(Poll),
  StopPoll,
  /// Spotlights `nick`'s message sent at `message_timestamp` (unix ms),
  /// whose text was `data` exactly as sent.
  Spotlight {
    nick: String,
    message_timestamp: i64,
    data: String,
  },
  /// Ends the spotlight with this key.
  Unspotlight(String),
  /// Removes the paid event with this uuid.
  RemoveEvent(String),
  /// Mutes yourself, by chat's game (`DEATH`).
  Die,
  /// Has the server reload its users.
  ReloadUsers,
}

/// A duration as the server reads it: nanoseconds (Go's `time.Duration`).
fn nanoseconds(duration: Duration) -> i64 {
  i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

impl Command {
  /// The event name it's sent as.
  pub fn name(&self) -> &'static str {
    match self {
      Self::Msg(_) => "MSG",
      Self::PrivMsg { .. } => "PRIVMSG",
      Self::CastVote(_) => "CASTVOTE",
      Self::Mute { .. } => "MUTE",
      Self::Unmute(_) => "UNMUTE",
      Self::Ban { .. } => "BAN",
      Self::Unban(_) => "UNBAN",
      Self::SubOnly(_) => "SUBONLY",
      Self::Broadcast(_) => "BROADCAST",
      Self::Pin(_) | Self::Unpin => "PIN",
      Self::AddPhrase(_) => "ADDPHRASE",
      Self::RemovePhrase(_) => "REMOVEPHRASE",
      Self::StartPoll(_) => "STARTPOLL",
      Self::StopPoll => "STOPPOLL",
      Self::Spotlight { .. } => "SPOTLIGHT",
      Self::Unspotlight(_) => "UNSPOTLIGHT",
      Self::RemoveEvent(_) => "REMOVEEVENT",
      Self::Die => "DIE",
      Self::ReloadUsers => "RELOADUSERS",
    }
  }

  /// The payload it's sent with.
  pub fn payload(&self) -> Value {
    match self {
      Self::Msg(data)
      | Self::Unmute(data)
      | Self::Unban(data)
      | Self::Broadcast(data)
      | Self::Pin(data)
      | Self::AddPhrase(data)
      | Self::RemovePhrase(data)
      | Self::Unspotlight(data)
      | Self::RemoveEvent(data) => json!({ "data": data }),
      Self::Unpin | Self::Die => json!({ "data": "" }),
      Self::StopPoll | Self::ReloadUsers => json!({}),
      Self::PrivMsg { nick, data } => json!({ "nick": nick, "data": data }),
      Self::CastVote(vote) => json!({ "vote": vote }),
      Self::Mute { nick, duration } => match duration {
        Some(duration) if !duration.is_zero() => {
          json!({ "data": nick, "duration": nanoseconds(*duration) })
        }
        _ => json!({ "data": nick }),
      },
      Self::Ban {
        nick,
        reason,
        length,
        ip,
      } => {
        let mut ban = json!({ "nick": nick, "reason": reason, "banip": ip });
        match length {
          BanLength::Permanent => ban["ispermanent"] = json!(true),
          BanLength::For(duration) => {
            ban["duration"] = json!(nanoseconds(*duration))
          }
        }
        ban
      }
      Self::SubOnly(SubOnly::Off) => json!({ "data": "off" }),
      Self::SubOnly(SubOnly::Tier(tier)) => {
        json!({ "data": "on", "tier": tier })
      }
      Self::StartPoll(poll) => json!({
        "weighted": poll.weighted,
        "hidden": poll.hidden,
        "time": u64::try_from(poll.time.as_millis()).unwrap_or(u64::MAX),
        "question": poll.question,
        "options": poll.options,
      }),
      Self::Spotlight {
        nick,
        message_timestamp,
        data,
      } => json!({
        "nick": nick,
        "messageTimestamp": message_timestamp,
        "data": data,
      }),
    }
  }

  /// The frame to send: `EVENT {json}`.
  pub fn encode(&self) -> String {
    format!("{} {}", self.name(), self.payload())
  }
}
