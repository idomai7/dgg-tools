//! A user's history in a channel: what the channel's journal has of them,
//! and the mod log's actions on them.
//!
//! Looked up by user id, which stays when they rename. The journal is read
//! whole on each lookup, with no index: kick-mod's journals hold only what
//! it heard while running. Its frames are read into events by the same code
//! as live chat; a file still being written is read up to its last complete
//! record. The messages a gap was filled with (its `history` records) are
//! read too, but those it has from the connection itself.
//!
//! Or, with the logger's `serve` in the config, the logger's answer is
//! read instead of the journal (see [`from_logger`]), in the same shape.

use {
  crate::{
    chat::{self, Named, What},
    modlog::{self, Entry},
  },
  ::kick_api::{realtime::Frame, web},
  ::logger_journal::{
    ReadError, Reader, Record, archive,
    layout::{channel_root, live_files},
    record::kick::v0::Body,
  },
  ::serde::{Deserialize, Serialize},
  ::std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
  },
};

/// The most messages a lookup returns: the newest.
pub const MAX_MESSAGES: usize = 500;

/// What's known of one user in one channel, each part oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct History {
  pub user_id: u64,
  /// The newest name seen, if any.
  pub username: Option<String>,
  /// Their messages, by when Kick says they were sent: the newest
  /// [`MAX_MESSAGES`].
  pub messages: Vec<Said>,
  /// How many older messages were left out.
  pub earlier: usize,
  /// Bans, timeouts and unbans on them.
  pub events: Vec<Happened>,
  /// The actions taken on them through kick-mod, refused ones too.
  pub actions: Vec<Entry>,
  /// Journal files that couldn't be read, and why.
  pub unreadable: Vec<String>,
}

/// One of their messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Said {
  /// When kick-mod received it, unix ms: for a recovered one, when the
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

impl Said {
  /// When it was sent, as near as is known.
  fn time(&self) -> i64 {
    self.at.unwrap_or(self.received)
  }
}

/// A message's deletion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deletion {
  /// When kick-mod received it, unix ms.
  pub received: i64,
  /// Whether Kick's AI moderation deleted it.
  pub ai: bool,
}

/// A ban, timeout or unban on them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Happened {
  /// When kick-mod received it, unix ms.
  pub received: i64,
  #[serde(flatten)]
  pub what: What,
}

/// The history of `user_id` in the channel `slug`, from Kick's journal
/// root `journal` and `modlog`, the mod log's contents.
pub fn read(
  journal: &Path,
  modlog: &str,
  slug: &str,
  user_id: u64,
) -> Result<History, String> {
  let mut history = History {
    user_id,
    username: None,
    messages: Vec::new(),
    earlier: 0,
    events: Vec::new(),
    actions: Vec::new(),
    unreadable: Vec::new(),
  };
  // A slug that can't be a directory's name was never journaled.
  if chat::journalable(slug) {
    let root = channel_root(journal, slug);
    let files = files(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    read_journal(&files, &mut history);
  }
  add_actions(&mut history, modlog, slug)?;
  Ok(history)
}

/// The history of `user_id` in the channel `slug`, from `logged`, the
/// logger's answer (none if it has no such channel), and `modlog`, the mod
/// log's contents.
pub fn from_logger(
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
    unreadable: Vec::new(),
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

/// A channel's journal files under its journal root: archives, then live
/// files.
fn files(root: &Path) -> io::Result<Vec<PathBuf>> {
  let mut files: Vec<_> = archive::all(root)?
    .iter()
    .map(|archive| archive.path(root))
    .collect();
  files.extend(live_files(root)?);
  Ok(files)
}

/// Adds what `files` have of the user to `history`.
fn read_journal(files: &[PathBuf], history: &mut History) {
  let user_id = history.user_id;
  let mut deletions = HashMap::new();
  // Messages from the history, kept unless the connection had them too.
  let mut recovered = Vec::new();
  // The newest name seen, by when it was received.
  let mut named: Option<(i64, String)> = None;
  let mut name = |received, username: &str| {
    if named.as_ref().is_none_or(|(at, _)| received >= *at) {
      named = Some((received, username.to_owned()));
    }
  };
  for path in files {
    let reader = match Reader::open(path, 0) {
      Ok(reader) => reader,
      Err(e) => {
        history
          .unreadable
          .push(format!("{}: {}", path.display(), why(&e)));
        continue;
      }
    };
    for entry in reader {
      let entry = match entry {
        Ok(entry) => entry,
        // Torn by a crash, or a kind this build doesn't know.
        Err(ReadError::Record { .. }) => continue,
        Err(e) => {
          history
            .unreadable
            .push(format!("{}: {}", path.display(), why(&e)));
          break;
        }
      };
      let Record::Kick0(record) = entry.record else {
        continue;
      };
      let received = record.receive_time;
      let raw = match record.body {
        Body::Frame(raw) => raw,
        Body::History { page, .. } => {
          // A page that doesn't decode is kept in the journal, as served.
          let Ok(page) = web::parse(&page) else {
            continue;
          };
          let theirs =
            page.messages.into_iter().filter(|m| m.sender.id == user_id);
          recovered.extend(theirs.map(|m| {
            let said = Said {
              received,
              at: m.created_at,
              id: m.id,
              content: m.content,
              deleted: None,
              recovered: true,
            };
            (m.sender.username, said)
          }));
          continue;
        }
        _ => continue,
      };
      let frame = Frame::new(record.connection, raw);
      for what in frame.events().filter_map(What::from_event) {
        match what {
          What::Message {
            id,
            at,
            sender,
            content,
          } if sender.id == user_id => {
            name(received, &sender.username);
            history.messages.push(Said {
              received,
              at,
              id,
              content,
              deleted: None,
              recovered: false,
            });
          }
          What::Deleted { id, ai } => {
            deletions.entry(id).or_insert(Deletion { received, ai });
          }
          What::Banned { ref user, .. } | What::Unbanned { ref user, .. }
            if user.id == user_id =>
          {
            name(received, &user.username);
            history.events.push(Happened { received, what });
          }
          _ => {}
        }
      }
    }
  }
  let mut ids: HashSet<String> = history
    .messages
    .iter()
    .map(|said| said.id.clone())
    .collect();
  for (username, said) in recovered {
    if ids.insert(said.id.clone()) {
      name(said.time(), &username);
      history.messages.push(said);
    }
  }
  history.messages.sort_by_key(Said::time);
  history.events.sort_by_key(|happened| happened.received);
  for said in &mut history.messages {
    said.deleted = deletions.get(&said.id).copied();
  }
  history.earlier = history.messages.len().saturating_sub(MAX_MESSAGES);
  history.messages.drain(..history.earlier);
  history.username = named.map(|(_, username)| username);
}

/// Why a file couldn't be read.
fn why(error: &ReadError) -> String {
  match error {
    ReadError::Io(e) => e.to_string(),
    ReadError::Header { .. } => "not a journal".into(),
    ReadError::Version(variant, version) => {
      let (major, minor) = (version.major, version.minor);
      format!("{variant:?} version {major}.{minor} isn't read")
    }
    ReadError::Record { offset, .. } => format!("a bad record at {offset}"),
  }
}
