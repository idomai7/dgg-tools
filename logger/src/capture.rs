//! Capture: dgg chat into the journal, and nothing else, so it stays small
//! and rarely needs upgrading.
//!
//! Instances hand off to each other through [`control`] status files: an
//! instance retires once a later-started one has been healthy for the
//! handoff grace period, so upgrading means starting the new binary. An
//! instance on another host takes over by pushing its status to that host's
//! `serve` ([`Peer`]).

use {
  crate::control::{self, Status},
  ::dgg_api::{Client, Item, Options},
  ::logger_journal::{
    Writer, layout::utc_day, record::v0::Body, writer::SYNC_INTERVAL,
  },
  ::std::{
    fmt::Display,
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

/// `YYYY-MM-DD HH:MM:SS.mmmZ` for unix ms `ms`, as log lines start.
pub fn timestamp(ms: i64) -> String {
  let of_day = ms.rem_euclid(86_400_000);
  format!(
    "{} {:02}:{:02}:{:02}.{:03}Z",
    utc_day(ms),
    of_day / 3_600_000,
    of_day / 60_000 % 60,
    of_day / 1000 % 60,
    of_day % 1000
  )
}

/// Writes a capture's own lines to stderr, each naming the instance. A
/// bridge's (a capture an install starts, with no supervisor to timestamp
/// its lines) are timestamped and marked too.
#[derive(Clone, Debug)]
pub struct Say {
  pub instance: String,
  pub bridge: bool,
}

impl Say {
  pub fn line(&self, message: impl Display) {
    if self.bridge {
      eprintln!(
        "{} {} (bridge): {message}",
        timestamp(now_ms()),
        self.instance
      );
    } else {
      eprintln!("{}: {message}", self.instance);
    }
  }
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
  /// Another host to push this instance's status to, so instances there
  /// hand off to this one.
  pub peer: Option<Peer>,
  /// Whether this is a bridge, for its log lines (see [`Say`]).
  pub bridge: bool,
}

/// Another host's `serve`.
#[derive(Clone, Debug)]
pub struct Peer {
  /// e.g. `http://old-host:8788`.
  pub url: String,
  pub token: String,
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
    peer,
    bridge,
  } = config;
  let say = Say {
    instance: instance.clone(),
    bridge,
  };
  let url = chat.url.clone();
  let control = control::dir(&journal_root);
  let mut writer = Writer::new(journal_root, instance.clone());
  let mut client = Client::spawn(chat);
  let mut sync = ::tokio::time::interval(SYNC_INTERVAL);
  let mut heartbeat = ::tokio::time::interval(handoff.interval);
  let mut connected = None;
  let mut health = Health::default();
  let pusher = peer.clone().map(|peer| {
    let control = control.clone();
    ::tokio::spawn(push_status(peer, control, say.clone(), handoff.interval))
  });
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
            say.line(format_args!("disconnected (#{connection}): {reason}"));
            writer.write(now, connection, Body::Close(reason))?;
          }
          Item::ConnectFailed { error, retry_in } => {
            say.line(format_args!(
              "connect failed: {error}; retrying in {retry_in:?}"
            ));
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
          build: Some(crate::BUILD.into()),
        };
        // Control files are advisory: a failed round (e.g. Windows refusing
        // a rename while another instance reads the file) is retried next
        // tick rather than stopping capture.
        match check_handoff(&control, &me, &handoff) {
          Ok(Some(successor)) => break Exit::HandedOff(successor),
          Ok(None) => {}
          Err(e) => say.line(format_args!("control: {e}")),
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
    say.line(format_args!("control: {e}"));
  }
  if let (Some(pusher), Some(peer)) = (pusher, peer) {
    pusher.abort();
    withdraw_status(&peer, &say).await;
  }
  Ok(exit)
}

fn peer_client() -> ::reqwest::Client {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  ::reqwest::Client::builder()
    .timeout(Duration::from_secs(5))
    .build()
    .expect("http client")
}

fn peer_url(peer: &Peer, instance: &str) -> String {
  format!("{}/control/{instance}", peer.url.trim_end_matches('/'))
}

/// Pushes this instance's status file to `peer` every `interval`, until
/// aborted. Failures are reported when they start and stop, not every
/// round: the peer's copy just goes stale, so its instances keep running.
async fn push_status(
  peer: Peer,
  control: PathBuf,
  say: Say,
  interval: Duration,
) {
  let client = peer_client();
  let instance = &say.instance;
  let url = peer_url(&peer, instance);
  let mut tick = ::tokio::time::interval(interval);
  let mut failing: Option<String> = None;
  loop {
    tick.tick().await;
    let path = control::path(&control, instance);
    let Ok(status) = ::tokio::fs::read_to_string(path).await else {
      continue;
    };
    let sent = client
      .put(&url)
      .bearer_auth(&peer.token)
      .body(control::push_body(&status, now_ms()))
      .send()
      .await;
    let error = match sent {
      Ok(response) if response.status().is_success() => None,
      Ok(response) => Some(format!("HTTP {}", response.status())),
      Err(e) => Some(e.to_string()),
    };
    match (&failing, error) {
      (None, Some(error)) => {
        say.line(format_args!("peer {}: {error}", peer.url));
        failing = Some(error);
      }
      (Some(_), None) => {
        say.line(format_args!("peer {}: pushing again", peer.url));
        failing = None;
      }
      _ => {}
    }
  }
}

/// Removes this instance's status from `peer`, best effort.
async fn withdraw_status(peer: &Peer, say: &Say) {
  let sent = peer_client()
    .delete(peer_url(peer, &say.instance))
    .bearer_auth(&peer.token)
    .send()
    .await;
  if let Err(e) = sent {
    say.line(format_args!("peer {}: {e}", peer.url));
  }
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
