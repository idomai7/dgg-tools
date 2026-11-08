//! Capture: dgg chat into the journal, and nothing else, so it stays small
//! and rarely needs upgrading.
//!
//! Instances hand off to each other through [`control`] status files: an
//! instance retires once a later-started one has been healthy for the
//! handoff grace period, so upgrading means starting the new binary.

use {
  crate::control::{self, Status},
  ::dgg_chat::{Client, Item, Options},
  ::journal::{Writer, record::v1::Body, writer::SYNC_INTERVAL},
  ::std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
  },
};

/// The [`Body::Close`] reason written when capture is told to stop.
pub const SHUTDOWN: &str = "shutdown";

/// Prefix of the [`Body::Close`] reason written when capture retires,
/// followed by the successor's instance id.
pub const HANDOFF: &str = "handoff to ";

/// An instance id unique to this run: `<host>-<start unix ms>`, with any
/// character a journal instance id can't hold replaced by `-`.
pub fn instance_id(host: &str, start_ms: i64) -> String {
  let host: String = host
    .chars()
    .map(|c| match c {
      'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => c,
      _ => '-',
    })
    .collect();
  let host = if host.is_empty() { "host" } else { &host };
  format!("{host}-{start_ms}")
}

/// Unix ms now.
pub fn now_ms() -> i64 {
  let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
  since.as_millis() as i64
}

/// How instances hand off to each other.
#[derive(Clone, Debug)]
pub struct Handoff {
  /// How long a successor must have been healthy before this instance
  /// retires: the overlap during which both write.
  pub grace: Duration,
  /// A status whose heartbeat is older than this is ignored (its instance
  /// probably crashed).
  pub stale_after: Duration,
  /// How often the status file is written and others' are read.
  pub interval: Duration,
  /// Never retire, for deliberately redundant instances.
  pub keep_running: bool,
}

impl Default for Handoff {
  fn default() -> Self {
    Self {
      grace: Duration::from_secs(30),
      stale_after: Duration::from_secs(5),
      interval: Duration::from_secs(1),
      keep_running: false,
    }
  }
}

pub struct Config {
  pub chat: Options,
  pub journal_root: PathBuf,
  pub instance: String,
  /// When this instance started, unix ms. Later-started instances win.
  pub started: i64,
  pub handoff: Handoff,
}

/// Why [`run`] returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
  /// `shutdown` completed.
  Shutdown,
  /// Retired in favour of this instance.
  HandedOff(String),
}

/// Whether the current connection has proven it's delivering chat.
#[derive(Default)]
struct Health {
  history: bool,
  message: bool,
  since: Option<i64>,
}

impl Health {
  fn saw(&mut self, event: &str, now: i64) {
    match event {
      "HISTORY" => self.history = true,
      "MSG" => self.message = true,
      _ => return,
    }
    if self.history && self.message && self.since.is_none() {
      self.since = Some(now);
    }
  }
}

/// Streams chat into the journal until `shutdown` completes or a successor
/// takes over, then closes the open connection's record, syncs, and removes
/// this instance's status file.
///
/// Each item is stamped with its receive time as it's taken off the client's
/// queue. A journal write error ends the run, since carrying on would
/// silently drop messages.
pub async fn run(
  config: Config,
  shutdown: impl Future<Output = ()>,
) -> io::Result<Exit> {
  let Config {
    chat,
    journal_root,
    instance,
    started,
    handoff,
  } = config;
  let url = chat.url.clone();
  let control = control::dir(&journal_root);
  let mut writer = Writer::new(journal_root, instance.clone());
  let mut client = Client::spawn(chat);
  let mut sync = ::tokio::time::interval(SYNC_INTERVAL);
  let mut heartbeat = ::tokio::time::interval(handoff.interval);
  let mut connected = None;
  let mut health = Health::default();
  ::tokio::pin!(shutdown);
  let exit = loop {
    ::tokio::select! {
      item = client.next() => {
        let Some(item) = item else { break Exit::Shutdown };
        let now = now_ms();
        match item {
          Item::Connected { connection } => {
            connected = Some(connection);
            health = Health::default();
            writer.write(now, connection, Body::Open(url.clone()))?;
          }
          Item::Frame(frame) => {
            health.saw(frame.event(), now);
            let connection = frame.connection;
            writer.write(now, connection, Body::Frame(frame.into_raw()))?;
          }
          Item::Binary { connection, data } => {
            writer.write(now, connection, Body::Binary(data))?;
          }
          Item::Disconnected { connection, reason } => {
            connected = None;
            health = Health::default();
            eprintln!("disconnected (#{connection}): {reason}");
            writer.write(now, connection, Body::Close(reason))?;
          }
          Item::ConnectFailed { error, retry_in } => {
            eprintln!("connect failed: {error}; retrying in {retry_in:?}");
          }
        }
      }
      _ = sync.tick() => writer.sync()?,
      _ = heartbeat.tick() => {
        let me = Status {
          instance: instance.clone(),
          started,
          heartbeat: now_ms(),
          healthy_since: health.since,
        };
        // Control files are advisory: a failed round (e.g. Windows refusing
        // a rename while another instance reads the file) is retried next
        // tick rather than stopping capture.
        match check_handoff(&control, &me, &handoff) {
          Ok(Some(successor)) => break Exit::HandedOff(successor),
          Ok(None) => {}
          Err(e) => eprintln!("control: {e}"),
        }
      }
      () = &mut shutdown => break Exit::Shutdown,
    }
  };
  if let Some(connection) = connected {
    let reason = match &exit {
      Exit::Shutdown => SHUTDOWN.to_owned(),
      Exit::HandedOff(successor) => format!("{HANDOFF}{successor}"),
    };
    writer.write(now_ms(), connection, Body::Close(reason))?;
  }
  writer.sync()?;
  if let Err(e) = control::remove(&control, &instance) {
    eprintln!("control: {e}");
  }
  Ok(exit)
}

/// Writes this instance's status, then returns the successor to hand off
/// to, if there is one and handing off is allowed.
fn check_handoff(
  control: &Path,
  me: &Status,
  handoff: &Handoff,
) -> io::Result<Option<String>> {
  control::write(control, me)?;
  if handoff.keep_running {
    return Ok(None);
  }
  let others = control::read_all(control)?;
  let successor = control::successor(
    me,
    &others,
    me.heartbeat,
    handoff.grace,
    handoff.stale_after,
  );
  Ok(successor.map(|status| status.instance.clone()))
}
