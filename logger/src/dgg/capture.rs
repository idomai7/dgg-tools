//! dgg's capture: its chat into the journal, and nothing else, so it stays
//! small and rarely needs upgrading. It hands off as every capture does
//! (see [`crate::capture`]).

use {
  crate::{
    capture::{Exit, HANDOFF, Handoff, Peer, SHUTDOWN, Say},
    control::{self, Status},
    dgg::{Dgg, channels::DESTINY},
    service::routes,
  },
  ::common::time::now_ms,
  ::dgg_api::chat::{Client, Item, Options},
  ::logger_journal::{Writer, record::v0::Body, writer::SYNC_INTERVAL},
  ::std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
  },
};

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
  let mut writer = Writer::capture(journal_root, instance.clone())?;
  let mut client = Client::spawn(chat);
  let mut sync = ::tokio::time::interval(SYNC_INTERVAL);
  let mut heartbeat = ::tokio::time::interval(handoff.interval);
  let mut connected = None;
  let mut health = Health::default();
  let mut older = control::Older::default();
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
            say.warn(format_args!("disconnected (#{connection}): {reason}"));
            writer.write(now, connection, Body::Close(reason))?;
          }
          Item::ConnectFailed { error, retry_in } => {
            say.warn(format_args!(
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
          listening: Some(url.clone()),
        };
        // Control files are advisory: a failed round (e.g. Windows refusing
        // a rename while another instance reads the file) is retried next
        // tick rather than stopping capture.
        match check_handoff(&control, &me, &handoff, &mut older) {
          Ok((successor, gone)) => {
            for line in gone {
              say.info(line);
            }
            if let Some(successor) = successor {
              break Exit::HandedOff(successor);
            }
          }
          Err(e) => say.warn(format_args!("control: {e}")),
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
    say.warn(format_args!("control: {e}"));
  }
  if let (Some(pusher), Some(peer)) = (pusher, peer) {
    pusher.abort();
    withdraw_status(&peer, &say).await;
  }
  Ok(exit)
}

fn peer_client(peer: &Peer) -> Result<::reqwest::Client, String> {
  peer.client(::reqwest::Client::builder().timeout(Duration::from_secs(5)))
}

/// Where `instance`'s status goes on `peer`: destiny.gg's channel's, the
/// one it captures.
fn peer_url(peer: &Peer, instance: &str) -> String {
  let base = peer.url.trim_end_matches('/');
  let at = routes::<Dgg>();
  format!("{base}{at}/channels/{DESTINY}/control/{instance}")
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
  let client = match peer_client(&peer) {
    Ok(client) => client,
    Err(e) => {
      say.error(format_args!("peer {}: {e}", peer.url));
      return;
    }
  };
  let instance = &say.instance;
  let mut tick = ::tokio::time::interval(interval);
  let mut failing: Option<String> = None;
  loop {
    tick.tick().await;
    let path = control::path(&control, instance);
    let Ok(status) = ::tokio::fs::read_to_string(path).await else {
      continue;
    };
    let sent = client
      .put(peer_url(&peer, instance))
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
        say.warn(format_args!("peer {}: {error}", peer.url));
        failing = Some(error);
      }
      (Some(_), None) => {
        say.info(format_args!("peer {}: pushing again", peer.url));
        failing = None;
      }
      _ => {}
    }
  }
}

/// Removes this instance's status from `peer`, best effort.
async fn withdraw_status(peer: &Peer, say: &Say) {
  let sent = match peer_client(peer) {
    Ok(client) => client
      .delete(peer_url(peer, &say.instance))
      .bearer_auth(&peer.token)
      .send()
      .await
      .map(drop)
      .map_err(|e| e.to_string()),
    Err(e) => Err(e),
  };
  if let Err(e) = sent {
    say.warn(format_args!("peer {}: {e}", peer.url));
  }
}

/// Writes this instance's status, then returns the successor to hand off
/// to, if there is one and handing off is allowed.
fn check_handoff(
  control: &Path,
  me: &Status,
  handoff: &Handoff,
  older: &mut control::Older,
) -> io::Result<(Option<String>, Vec<String>)> {
  control::write(control, me)?;
  let others = control::read_all(control)?;
  let gone = older.gone(me, &others);
  if handoff.keep_running {
    return Ok((None, gone));
  }
  let successor = control::successor(
    me,
    &others,
    me.heartbeat,
    handoff.grace,
    handoff.stale_after,
  );
  Ok((successor.map(|status| status.instance.clone()), gone))
}
