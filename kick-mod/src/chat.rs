//! Live chat: a realtime connection per channel, with what arrives on it
//! turned into [`Update`]s for the page.
//!
//! Each channel with a chatroom id gets a [`Client`] of its own,
//! subscribed to `chatrooms.<chatroom id>.v2`, so one channel's
//! connection failing leaves the others. Kick keeps no history there:
//! what's sent while a connection is down is lost.
//!
//! So when a connection comes back after a drop, the gap's messages are
//! [filled](fill) from Kick's chat history, if the channel's id is known:
//! back to the last thing heard before the drop, at most [`MAX_PAGES`]
//! pages. Only messages come back; deletions, bans and the rest in the gap
//! stay missing.
//!
//! What each connection receives is journaled as it arrives, raw, in the
//! logger's journal format (Kick's variant): under [`Journal::root`], each
//! channel's in a journal root of its own, as
//! `<slug>/live/<day>/<instance>.journal`. The history pages a gap was
//! filled from are journaled too, raw, on the connection that came back.

use {
  crate::channels::Channels,
  ::common::time::now_ms,
  ::kick_api::{
    realtime::{
      Client, Event, Frame, Item, Options,
      centrifugo::{push, reply},
      event::User,
    },
    web::{self, Web},
  },
  ::logger_journal::{
    layout::channel_root,
    record::kick::v0::Body,
    writer::{SYNC_INTERVAL, kick::Writer},
  },
  ::serde::Serialize,
  ::std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
  },
  ::tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
    time::interval,
  },
};

/// How far a page may fall behind before it misses updates.
const BACKLOG: usize = 1024;

/// The most pages of history a gap is filled from: 250 messages.
pub const MAX_PAGES: usize = 10;

/// How long before the last thing heard a gap is filled from (ms): Kick's
/// times are whole seconds, and its clock isn't ours.
const MARGIN: i64 = 5_000;

/// How many messages' ids are kept, to tell a filled message from one
/// received.
const SEEN: usize = 4096;

pub const DEFAULT_JOURNAL: &str = "data/kick-mod/journal";

/// Where chat is journaled.
#[derive(Clone, Debug)]
pub struct Journal {
  /// Kick's journal root: each channel's is `<root>/<slug>`.
  pub root: PathBuf,
  /// Written on every line, and naming the files: one per run, so runs
  /// never share a file.
  pub instance: String,
}

impl Journal {
  /// A journal under `root`, for a run that started at `start_ms`: its
  /// instance is `kick-mod-<start_ms>`.
  pub fn new(root: impl Into<PathBuf>, start_ms: i64) -> Self {
    Self {
      root: root.into(),
      instance: format!("kick-mod-{start_ms}"),
    }
  }
}

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
  /// Messages sent while the connection was down, recovered from Kick's
  /// chat history: each a [`What::Message`], oldest first. `partial` if
  /// the gap went back further than was fetched, or fetching failed, with
  /// `error` why.
  Filled {
    messages: Vec<What>,
    partial: bool,
    error: Option<String>,
  },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
  /// Not listened to: no chatroom id.
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

  /// The connection's state after `item`, if it changed it.
  pub fn from_item(item: &Item) -> Option<Self> {
    let (state, detail) = match item {
      Item::Subscribed { .. } => (State::Live, None),
      Item::Disconnected { code, reason, .. } => {
        let code = code.map(|code| format!("{code} ")).unwrap_or_default();
        (State::Reconnecting, Some(format!("{code}{reason}")))
      }
      Item::ConnectFailed {
        stage,
        error,
        retry_in,
      } => {
        let again = ::common::format::duration(retry_in.as_millis() as i64);
        let detail = format!("{stage}: {error}; again in {again}");
        (State::Reconnecting, Some(detail))
      }
      Item::Unsubscribed {
        code,
        reason,
        resubscribing,
        ..
      } => {
        let state = if *resubscribing {
          State::Reconnecting
        } else {
          State::Stopped
        };
        (state, Some(format!("{code} {reason}")))
      }
      Item::Stopped { code, reason } => {
        (State::Stopped, Some(format!("{code} {reason}")))
      }
      Item::Connected { .. } | Item::Frame(_) | Item::Binary { .. } => {
        return None;
      }
    };
    Some(Self::Status { state, detail })
  }
}

/// What `item` puts in the journal, on which connection, with `open` the
/// connection that's open.
fn journaled(item: &Item, open: &mut Option<u64>) -> Option<(u64, Body)> {
  Some(match item {
    Item::Connected {
      connection,
      endpoint,
    } => {
      *open = Some(*connection);
      (*connection, Body::Open(endpoint.clone()))
    }
    Item::Frame(frame) => (frame.connection, Body::Frame(frame.raw().into())),
    Item::Binary { connection, data } => {
      (*connection, Body::Binary(data.clone()))
    }
    Item::Disconnected {
      connection,
      code,
      reason,
    } => {
      *open = None;
      let code = code.map(|code| format!("{code} ")).unwrap_or_default();
      (*connection, Body::Close(format!("{code}{reason}")))
    }
    // Ends the open connection, with no disconnect of its own.
    Item::Stopped { code, reason } => {
      (open.take()?, Body::Close(format!("{code} {reason}")))
    }
    // In frames, or before a connection opened.
    Item::ConnectFailed { .. }
    | Item::Subscribed { .. }
    | Item::Unsubscribed { .. } => return None,
  })
}

/// The events published in `frame`.
pub fn events(frame: &Frame) -> impl Iterator<Item = Event> + '_ {
  frame.replies().filter_map(|reply| match reply.ok()?.body? {
    reply::Body::Push(push::Push {
      body: Some(push::Body::Publication(publication)),
      ..
    }) => Event::from_publication(&publication.data),
    _ => None,
  })
}

/// What the connections share: where updates go, and each channel's
/// status, for pages that start watching later.
struct Hub {
  updates: broadcast::Sender<Arc<Update>>,
  statuses: Mutex<BTreeMap<String, Arc<Update>>>,
}

impl Hub {
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

/// Every channel's connection. Dropping it closes them.
pub struct Chat {
  hub: Arc<Hub>,
  tasks: Vec<JoinHandle<()>>,
}

impl Chat {
  /// Connects to each of `channels` with a chatroom id, with `options`
  /// for each but its channels, journaling each in `journal`. Gaps are
  /// filled through `web`, for the channels whose id is known.
  pub fn start(
    channels: &Channels,
    options: &Options,
    journal: &Journal,
    web: Option<Arc<Web>>,
  ) -> Self {
    let hub = Arc::new(Hub {
      updates: broadcast::channel(BACKLOG).0,
      statuses: Mutex::default(),
    });
    let mut tasks = Vec::new();
    for (slug, channel) in channels {
      let Some(chatroom) = channel.chatroom_id else {
        let detail = "no chatroom id: enter it in channels.json".into();
        let off = What::Status {
          state: State::Off,
          detail: Some(detail),
        };
        hub.publish(slug, off);
        continue;
      };
      let connecting = What::Status {
        state: State::Connecting,
        detail: None,
      };
      hub.publish(slug, connecting);
      let client = Client::spawn(Options {
        channels: vec![format!("chatrooms.{chatroom}.v2")],
        ..options.clone()
      });
      let writer = if journalable(slug) {
        let root = channel_root(&journal.root, slug);
        Some(Writer::new(root, &journal.instance, slug))
      } else {
        eprintln!("{slug}: not journaled, as it can't be a directory's name");
        None
      };
      let filler = web.clone().zip(channel.channel_id);
      let listen = listen(slug.clone(), client, hub.clone(), writer, filler);
      tasks.push(::tokio::spawn(listen));
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

/// What a gap's fill fetched.
#[derive(Debug)]
struct Fill {
  /// The connection that came back.
  connection: u64,
  /// The gap's start, in Kick's time (unix ms), margin included.
  since: i64,
  back: web::Back,
}

/// Fetches the channel `channel_id`'s history back to `since` (unix ms),
/// at most [`MAX_PAGES`] pages, for the gap before `connection`.
async fn fill(
  web: Arc<Web>,
  channel_id: u64,
  connection: u64,
  since: i64,
) -> Fill {
  Fill {
    connection,
    since,
    back: web.back_to(channel_id, since, MAX_PAGES).await,
  }
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

/// A channel's connection's state between items.
#[derive(Default)]
struct Listening {
  /// The connection that's open, for a stop to close.
  open: Option<u64>,
  /// The connection last opened, for a fill to be journaled on.
  connection: Option<u64>,
  /// When (unix ms) something was last heard while subscribed.
  heard: Option<i64>,
  /// When something was last heard before a drop, until it's filled.
  gap: Option<i64>,
  seen: Seen,
}

async fn listen(
  slug: String,
  mut client: Client,
  hub: Arc<Hub>,
  mut journal: Option<Writer>,
  filler: Option<(Arc<Web>, u64)>,
) {
  let mut sync = interval(SYNC_INTERVAL);
  let mut state = Listening::default();
  let (filled, mut fills) = mpsc::unbounded_channel::<Fill>();
  let write = |journal: &mut Option<Writer>, connection, body| {
    if let Some(writer) = journal
      && let Err(e) = writer.write(now_ms(), connection, body)
    {
      eprintln!("{slug}: no longer journaled: {e}");
      *journal = None;
    }
  };
  loop {
    let item = ::tokio::select! {
      item = client.next() => item,
      Some(mut fill) = fills.recv() => {
        for (cursor, page) in ::std::mem::take(&mut fill.back.pages) {
          write(&mut journal, fill.connection, Body::History { cursor, page });
        }
        hub.publish(&slug, filled_in(fill, &mut state.seen));
        continue;
      }
      _ = sync.tick() => {
        if let Some(writer) = &mut journal
          && let Err(e) = writer.sync()
        {
          eprintln!("{slug}: no longer journaled: {e}");
          journal = None;
        }
        continue;
      }
    };
    let Some(item) = item else { break };
    let now = now_ms();
    if let Some((connection, body)) = journaled(&item, &mut state.open) {
      write(&mut journal, connection, body);
    }
    match &item {
      Item::Connected { connection, .. } => {
        state.connection = Some(*connection)
      }
      Item::Frame(_) => state.heard = Some(now),
      Item::Disconnected { .. }
      | Item::Unsubscribed {
        resubscribing: true,
        ..
      } => state.gap = state.gap.or(state.heard.take()),
      Item::Subscribed { .. } => {
        if let Some(heard) = state.gap.take()
          && let Some((web, channel_id)) = &filler
          && let Some(connection) = state.connection
        {
          let (web, channel_id) = (web.clone(), *channel_id);
          let filled = filled.clone();
          ::tokio::spawn(async move {
            let fill = fill(web, channel_id, connection, heard - MARGIN).await;
            // None waiting is fine: the channel's gone.
            let _ = filled.send(fill);
          });
        }
        state.heard = Some(now);
      }
      _ => {}
    }
    match &item {
      Item::Frame(frame) => {
        for what in events(frame).filter_map(What::from_event) {
          if let What::Message { id, .. } = &what {
            state.seen.insert(id);
          }
          hub.publish(&slug, what);
        }
      }
      item => {
        if let Some(status) = What::from_item(item) {
          hub.publish(&slug, status);
        }
      }
    }
  }
}

/// What the page is told of `fill`: its messages from the gap, oldest
/// first, but those already `seen`.
fn filled_in(fill: Fill, seen: &mut Seen) -> What {
  let mut messages: Vec<_> = fill
    .back
    .messages
    .into_iter()
    .filter(|m| m.created_at.is_none_or(|at| at > fill.since))
    .filter(|m| seen.insert(&m.id))
    .collect();
  messages.reverse();
  messages.sort_by_key(|m| m.created_at);
  What::Filled {
    messages: messages
      .into_iter()
      .map(|m| What::message(m.id, m.created_at, m.sender, m.content))
      .collect(),
    partial: !fill.back.complete,
    error: fill.back.error,
  }
}
