//! A user's history in a channel: what the logger has of them (see
//! [`logger`](crate::logger)), and the mod log's actions on them.
//!
//! Looked up by user id, which stays when they rename.

use {
  crate::{
    chat::{Named, What},
    modlog::{self, Entry},
  },
  ::serde::{Deserialize, Serialize},
};

/// What's known of one user in one channel, each part oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct History {
  pub user_id: u64,
  /// The newest name seen, if any.
  pub username: Option<String>,
  /// Their messages, by when Kick says they were sent: the newest 500.
  pub messages: Vec<Said>,
  /// How many older messages were left out.
  pub earlier: usize,
  /// Bans, timeouts and unbans on them.
  pub events: Vec<Happened>,
  /// The actions taken on them through kick-mod, refused ones too.
  pub actions: Vec<Entry>,
}

/// One of their messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Said {
  /// When the logger received it, unix ms: for a recovered one, when the
  /// history was fetched.
  pub received: i64,
  /// When Kick says it was sent, unix ms.
  pub at: Option<i64>,
  pub id: String,
  /// Emotes inline as `[emote:<id>:<name>]`.
  pub content: String,
  pub deleted: Option<Deletion>,
  /// Whether it was recovered from Kick's chat history, to fill a gap.
  pub recovered: bool,
}

/// A message's deletion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deletion {
  /// When the logger received it, unix ms.
  pub received: i64,
  /// Whether Kick's AI moderation deleted it.
  pub ai: bool,
}

/// A ban, timeout or unban on them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Happened {
  /// When the logger received it, unix ms.
  pub received: i64,
  #[serde(flatten)]
  pub what: What,
}

/// The history of `user_id` in the channel `slug`, from `logged`, the
/// logger's answer (none if it has no such channel), and `modlog`, the mod
/// log's contents.
pub fn read(
  logged: Option<&str>,
  modlog: &str,
  slug: &str,
  user_id: u64,
) -> Result<History, String> {
  let logged: Logged = match logged {
    Some(text) => ::serde_json::from_str(text)
      .map_err(|e| format!("the logger's answer: {e}"))?,
    None => Logged::default(),
  };
  let named = Named {
    id: user_id,
    username: logged.username.clone().unwrap_or_default(),
  };
  let events = logged.events.into_iter().filter_map(|event| {
    let what = match event.kind.as_str() {
      "ban" | "timeout" => What::Banned {
        user: named.clone(),
        by: event.by,
        permanent: event.permanent.unwrap_or(event.kind == "ban"),
        minutes: event.minutes,
        until: event.until,
      },
      "unban" => What::Unbanned {
        user: named.clone(),
        by: event.by,
      },
      // From a newer logger.
      _ => return None,
    };
    Some(Happened {
      received: event.received,
      what,
    })
  });
  let messages = logged.messages.into_iter().map(|message| Said {
    received: message.received,
    at: message.sent,
    id: message.id,
    content: message.content,
    deleted: message.deleted,
    recovered: message.source == "history",
  });
  let mut history = History {
    user_id,
    username: logged.username,
    messages: messages.collect(),
    earlier: logged.earlier,
    events: events.collect(),
    actions: Vec::new(),
  };
  add_actions(&mut history, modlog, slug)?;
  Ok(history)
}

/// The logger's answer, as far as it's read.
#[derive(Default, Deserialize)]
struct Logged {
  username: Option<String>,
  messages: Vec<LoggedMessage>,
  earlier: usize,
  events: Vec<LoggedEvent>,
}

#[derive(Deserialize)]
struct LoggedMessage {
  id: String,
  sent: Option<i64>,
  received: i64,
  content: String,
  /// `live`, or `history` if only a gap's history page had it.
  source: String,
  deleted: Option<Deletion>,
}

#[derive(Deserialize)]
struct LoggedEvent {
  received: i64,
  /// `ban`, `timeout` or `unban`.
  kind: String,
  by: Option<String>,
  minutes: Option<u64>,
  until: Option<i64>,
  permanent: Option<bool>,
}

/// Adds the mod log's actions on the user in channel `slug` to `history`,
/// from `modlog`, its contents.
fn add_actions(
  history: &mut History,
  modlog: &str,
  slug: &str,
) -> Result<(), String> {
  let user_id = history.user_id;
  let entries = modlog::parse(modlog).map_err(|e| format!("mod log: {e}"))?;
  history.actions = entries
    .into_iter()
    .filter(|entry| entry.asked.channel == slug)
    .filter(|entry| entry.asked.user.id == user_id)
    .collect();
  if history.username.is_none() {
    history.username = history
      .actions
      .last()
      .map(|entry| entry.asked.user.username.clone());
  }
  Ok(())
}
