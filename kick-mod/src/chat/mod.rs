//! Live chat, from the logger's `serve` (see [`logger`]), which captures
//! and journals Kick's: a stream per channel, with what arrives on it
//! turned into [`Update`]s for the page.

pub mod logger;

use {
  crate::{channels::Channels, logger::Client as Logger},
  ::kick_api::realtime::{Event, event::User},
  ::serde::Serialize,
  ::std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
  },
  ::tokio::{sync::broadcast, task::JoinHandle},
};

/// How far a page may fall behind before it misses updates.
const BACKLOG: usize = 1024;

/// How many messages' ids are kept, to tell a filled message from one
/// received.
const SEEN: usize = 4096;

/// Something that happened in a channel, by its slug.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Update {
  pub slug: String,
  #[serde(flatten)]
  pub what: What,
}

/// What happened, as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
  /// The channel's connection, now.
  Status {
    state: State,
    detail: Option<String>,
  },
  Message {
    id: String,
    /// Unix ms, when Kick says.
    at: Option<i64>,
    sender: Sender,
    /// Emotes inline as `[emote:<id>:<name>]`.
    content: String,
  },
  /// A message was deleted: by Kick's AI moderation if `ai`.
  Deleted {
    id: String,
    ai: bool,
  },
  /// A ban if `permanent`, otherwise a timeout of `minutes`, until `until`
  /// (unix ms).
  Banned {
    user: Named,
    by: Option<String>,
    permanent: bool,
    minutes: Option<u64>,
    until: Option<i64>,
  },
  Unbanned {
    user: Named,
    by: Option<String>,
  },
  Cleared,
  /// Messages sent while the capture's connection was down, recovered
  /// from Kick's chat history: each a [`What::Message`], oldest first.
  Filled {
    messages: Vec<What>,
  },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
  /// Not listened to: a slug the logger can't serve.
  Off,
  Connecting,
  Live,
  /// Down for now, and being retried.
  Reconnecting,
  /// Down, and not retried.
  Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Sender {
  pub id: u64,
  pub username: String,
  /// Such as `#00FF00`.
  pub color: Option<String>,
  /// Their types, such as `moderator`.
  pub badges: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Named {
  pub id: u64,
  pub username: String,
}

impl From<User> for Named {
  fn from(user: User) -> Self {
    Self {
      id: user.id,
      username: user.username,
    }
  }
}

impl What {
  /// What the page shows of `event`, if anything.
  pub fn from_event(event: Event) -> Option<Self> {
    Some(match event {
      Event::ChatMessageEvent(message) => Self::message(
        message.id,
        message.created_at,
        message.sender,
        message.content,
      ),
      Event::MessageDeletedEvent(deleted) => Self::Deleted {
        id: deleted.message.id,
        ai: deleted.ai_moderated.unwrap_or(false),
      },
      Event::UserBannedEvent(banned) => Self::Banned {
        user: banned.user.into(),
        by: banned.banned_by.map(|by| by.username),
        permanent: banned.permanent,
        minutes: banned.duration,
        until: banned.expires_at,
      },
      Event::UserUnbannedEvent(unbanned) => Self::Unbanned {
        user: unbanned.user.into(),
        by: unbanned.unbanned_by.map(|by| by.username),
      },
      Event::ChatroomClearEvent(_) => Self::Cleared,
      _ => return None,
    })
  }

  /// A message, from Kick's fields of it.
  fn message(
    id: String,
    at: Option<i64>,
    sender: User,
    content: String,
  ) -> Self {
    let identity = sender.identity;
    Self::Message {
      id,
      at,
      sender: Sender {
        id: sender.id,
        username: sender.username,
        color: identity.as_ref().and_then(|i| i.color.clone()),
        badges: identity
          .map(|i| i.badges.into_iter().map(|b| b.r#type).collect())
          .unwrap_or_default(),
      },
      content,
    }
  }
}

/// What the streams share: where updates go, and each channel's
/// status, for pages that start watching later.
struct Hub {
  updates: broadcast::Sender<Arc<Update>>,
  statuses: Mutex<BTreeMap<String, Arc<Update>>>,
}

impl Hub {
  fn new() -> Arc<Self> {
    Arc::new(Self {
      updates: broadcast::channel(BACKLOG).0,
      statuses: Mutex::default(),
    })
  }

  fn status(&self, slug: &str, state: State, detail: Option<String>) {
    self.publish(slug, What::Status { state, detail });
  }

  fn publish(&self, slug: &str, what: What) {
    let update = Arc::new(Update {
      slug: slug.into(),
      what,
    });
    if let What::Status { .. } = update.what {
      let mut statuses = self.statuses.lock().unwrap();
      statuses.insert(slug.into(), update.clone());
    }
    // None watching is fine.
    let _ = self.updates.send(update);
  }
}

/// Every channel's stream. Dropping it closes them.
pub struct Chat {
  hub: Arc<Hub>,
  tasks: Vec<JoinHandle<()>>,
}

impl Chat {
  /// Follows each of `channels` through `logger`.
  pub fn start(channels: &Channels, logger: &Logger) -> Self {
    let hub = Hub::new();
    let mut tasks = Vec::new();
    for slug in channels.keys() {
      if !journalable(slug) {
        let detail = "not a slug the logger serves".into();
        hub.status(slug, State::Off, Some(detail));
        continue;
      }
      hub.status(slug, State::Connecting, None);
      let follow =
        self::logger::follow(slug.clone(), logger.clone(), hub.clone());
      tasks.push(::tokio::spawn(follow));
    }
    Self { hub, tasks }
  }

  /// Each channel's status now, and the updates after.
  pub fn watch(&self) -> (Vec<Arc<Update>>, broadcast::Receiver<Arc<Update>>) {
    // Subscribed first, so nothing falls between; a status seen twice is
    // harmless.
    let later = self.hub.updates.subscribe();
    let now = self
      .hub
      .statuses
      .lock()
      .unwrap()
      .values()
      .cloned()
      .collect();
    (now, later)
  }
}

impl Drop for Chat {
  fn drop(&mut self) {
    for task in &self.tasks {
      task.abort();
    }
  }
}

/// Whether `slug` is safe as a directory's name, as Kick's slugs are.
pub fn journalable(slug: &str) -> bool {
  !slug.is_empty()
    && slug
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

/// The ids of the messages seen lately, the newest [`SEEN`].
#[derive(Default)]
struct Seen {
  ids: HashSet<String>,
  order: VecDeque<String>,
}

impl Seen {
  /// Notes `id`: whether it's new.
  fn insert(&mut self, id: &str) -> bool {
    if !self.ids.insert(id.into()) {
      return false;
    }
    self.order.push_back(id.into());
    if self.order.len() > SEEN
      && let Some(oldest) = self.order.pop_front()
    {
      self.ids.remove(&oldest);
    }
    true
  }
}
