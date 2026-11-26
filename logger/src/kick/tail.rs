//! Following a Kick channel's live files as they're written, for `serve`'s
//! live chat, apart from the capture process.
//!
//! A [`Tail`] starts at the end of what's written, and each
//! [`Tail::read`] reads what's been written since: every live file of the
//! channel, a new one (another instance's, or the next day's) from its
//! start. Records still being written are left for the next look.
//!
//! When captures overlap (during a handoff, or on several hosts' files
//! synced here) the same event is in more than one file: an event Kick
//! gave an id (a message, a deletion, a ban or an unban) is passed on only
//! the first time it's seen.

use {
  ::common::fs::MissingOk,
  ::kick_api::realtime::{
    Event, Frame,
    centrifugo::{push, reply},
  },
  ::logger_journal::{
    ReadError, Reader, Record,
    layout::live_files,
    record::{END, kick::v0::Body},
  },
  ::std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
  },
};

/// How many events' ids are kept, to pass each on once.
const SEEN: usize = 4096;

/// What was written, in the order each file has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
  /// When it was received, unix ms.
  pub received: i64,
  /// The capture instance that wrote it.
  pub instance: String,
  /// The connection, within the instance.
  pub connection: u64,
  pub what: What,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
  /// A connection opened, to `endpoint`.
  Open { endpoint: String },
  /// A connection closed, with why.
  Close { reason: String },
  /// An event Kick published: its name and its data, both as Kick sent
  /// them.
  Event { event: String, data: String },
  /// A page of chat history a gap was filled with, as Kick sent it.
  History { page: String },
}

/// One channel's live files, followed.
pub struct Tail {
  root: PathBuf,
  /// How far each file has been read.
  offsets: HashMap<PathBuf, u64>,
  seen: VecDeque<String>,
  known: HashSet<String>,
}

impl Tail {
  /// Follows the channel whose journal root is `root`, from the end of
  /// what's written now.
  pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
    let root = root.into();
    let mut offsets = HashMap::new();
    for file in live_files(&root)? {
      // Compacted away since it was listed: nothing of it to follow.
      if let Some(end) = last_end(&file).missing_ok()? {
        offsets.insert(file, end);
      }
    }
    Ok(Self {
      root,
      offsets,
      seen: VecDeque::new(),
      known: HashSet::new(),
    })
  }

  /// What's been written since the last look.
  pub fn read(&mut self) -> io::Result<Vec<Written>> {
    let files = live_files(&self.root)?;
    // Compacted away.
    self.offsets.retain(|file, _| files.contains(file));
    let mut written = Vec::new();
    for file in files {
      let offset = self.offsets.get(&file).copied().unwrap_or(0);
      let mut reader = match Reader::open(&file, offset) {
        Ok(reader) => reader,
        Err(ReadError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
          continue;
        }
        Err(ReadError::Io(e)) => return Err(e),
        // Not a file this build reads: nothing of it is passed on.
        Err(_) => {
          if let Some(metadata) = file.metadata().missing_ok()? {
            self.offsets.insert(file, metadata.len());
          }
          continue;
        }
      };
      for entry in reader.by_ref() {
        let record = match entry {
          Ok(entry) => match entry.record {
            Record::Kick0(record) => record,
            Record::V0(_) => continue,
          },
          Err(ReadError::Record { .. }) => continue,
          Err(ReadError::Io(e)) => return Err(e),
          Err(e) => return Err(io::Error::other(format!("{e:?}"))),
        };
        let (received, instance, connection) =
          (record.receive_time, record.instance, record.connection);
        let mut add = |what| {
          written.push(Written {
            received,
            instance: instance.clone(),
            connection,
            what,
          })
        };
        match record.body {
          Body::Open(endpoint) => add(What::Open { endpoint }),
          Body::Close(reason) => add(What::Close { reason }),
          Body::History { page, .. } => add(What::History { page }),
          Body::Frame(raw) => {
            let frame = Frame::new(connection, raw);
            for (event, data) in publications(&frame) {
              if self.first_time(&event, &data) {
                add(What::Event { event, data });
              }
            }
          }
          Body::Binary(_) => {}
        }
      }
      self.offsets.insert(file, reader.offset());
    }
    Ok(written)
  }

  /// Whether the event with `data` hasn't been passed on yet: always, for
  /// one Kick gave no id.
  fn first_time(&mut self, event: &str, data: &str) -> bool {
    let Some(id) = id(event, data) else {
      return true;
    };
    if !self.known.insert(id.clone()) {
      return false;
    }
    self.seen.push_back(id);
    if self.seen.len() > SEEN
      && let Some(oldest) = self.seen.pop_front()
    {
      self.known.remove(&oldest);
    }
    true
  }
}

/// Each event published in `frame`: its name and data, as Kick sent them.
fn publications(frame: &Frame) -> Vec<(String, String)> {
  frame
    .replies()
    .filter_map(|reply| match reply.ok()?.body? {
      reply::Body::Push(push::Push {
        body: Some(push::Body::Publication(publication)),
        ..
      }) => {
        let data = &publication.data;
        let event = data.get("event")?.as_str()?.to_owned();
        let data = data.get("data")?.as_str()?.to_owned();
        Some((event, data))
      }
      _ => None,
    })
    .collect()
}

/// The id that tells this event apart from others of its kind, if Kick
/// gave it one.
fn id(event: &str, data: &str) -> Option<String> {
  Some(match Event::parse(event, data) {
    Event::ChatMessageEvent(message) => format!("message {}", message.id),
    Event::MessageDeletedEvent(deleted) => {
      format!("deleted {}", deleted.message.id)
    }
    Event::UserBannedEvent(banned) => format!("banned {}", banned.id?),
    Event::UserUnbannedEvent(unbanned) => {
      format!("unbanned {}", unbanned.id?)
    }
    _ => return None,
  })
}

/// The offset just past the last complete record in the journal file at
/// `path`, or 0 (the start, after its header) if it has none: where to
/// start following it.
fn last_end(path: &Path) -> io::Result<u64> {
  let mut file = File::open(path)?;
  let size = file.metadata()?.len();
  const CHUNK: u64 = 64 << 10;
  let mut end = size;
  // A record's end may straddle chunks: each overlaps the next by a byte.
  while end > 0 {
    let start = end.saturating_sub(CHUNK);
    let mut chunk = vec![0; (end - start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut chunk)?;
    if let Some(at) = chunk.windows(END.len()).rposition(|w| w == END) {
      return Ok(start + (at + END.len()) as u64);
    }
    if start == 0 {
      break;
    }
    end = start + 1;
  }
  Ok(0)
}
