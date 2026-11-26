//! dgg's capture: a channel's chat into its journal, and nothing else, so
//! it stays small and rarely needs upgrading. It hands off as every capture
//! does (see [`crate::capture`]).
//!
//! Given the list's file, it's read again every [`Config::reread_every`]:
//! if the channel's URL or Origin changed, it connects again with them,
//! numbering its connections on; if the channel's gone, it stops, its
//! connection closed in its journal ([`Exit::Removed`]). A list that can't
//! be read changes nothing.

use {
  crate::{
    capture::{Exit, HANDOFF, Handoff, Peer, SHUTDOWN, Say, check_handoff},
    control::{self, Status},
    dgg::{
      Dgg,
      channels::{self, Channel},
    },
    service::routes,
  },
  ::common::time::now_ms,
  ::dgg_api::chat::{Client, Item, Options},
  ::logger_journal::{
    Variant, Writer, record::v0::Body, writer::SYNC_INTERVAL,
  },
  ::std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
  },
  ::tokio::time::{Instant, interval_at},
};

/// Why a channel removed from the list stopped, in its journal.
pub const REMOVED: &str = "removed from the channel list";

/// Why a channel whose URL or Origin changed stopped, in its journal,
/// before it connects again.
pub const CHANGED: &str = "its URL or Origin changed in the channel list";

pub struct Config {
  /// The channel captured.
  pub channel: Channel,
  /// The list's file, to follow; with none, `channel` stays as it is.
  pub list: Option<PathBuf>,
  /// How often the list is read again.
  pub reread_every: Duration,
  /// How it connects, but for its URL and Origin: the channel's.
  pub chat: Options,
  /// The channel's directory.
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

/// Whether the current connection has proven it's delivering chat: once
/// it has delivered `HISTORY`, which the server sends once it has joined
/// the chat. Not a `MSG` too, which a quiet chat may not send for hours.
/// Staying healthy for a handoff's grace proves the rest: health is lost
/// with the connection, and a connection that goes silent is dropped.
#[derive(Default)]
struct Health {
  since: Option<i64>,
}

impl Health {
  fn saw(&mut self, event: &str, now: i64) {
    if event == "HISTORY" && self.since.is_none() {
      self.since = Some(now);
    }
  }
}

/// What reading the list again calls for.
enum Change {
  /// Connect again, to this.
  Changed(Channel),
  Removed,
}

/// What `list` says of `channel` now, if it changed. A list that can't be
/// read changes nothing.
fn reread(list: &Path, channel: &Channel, say: &Say) -> Option<Change> {
  let listed = match channels::load(list) {
    Ok(listed) => listed,
    Err(e) => {
      say.warn(format_args!("{e}; capturing as before"));
      return None;
    }
  };
  match listed.into_iter().find(|c| c.name == channel.name) {
    None => Some(Change::Removed),
    Some(now) if now != *channel => Some(Change::Changed(now)),
    Some(_) => None,
  }
}

/// A client for `channel`, connecting as `chat` says otherwise.
fn connect(chat: &Options, channel: &Channel) -> Client {
  Client::spawn(Options {
    url: channel.url.clone(),
    origin: channel.origin.clone(),
    ..chat.clone()
  })
}

/// Streams the channel's chat into its journal until `shutdown` completes,
/// a successor takes over or the channel leaves the list, then closes the
/// open connection's record, syncs, and removes this instance's status
/// file.
///
/// Each item is stamped with its receive time as it's taken off the client's
/// queue. A journal write error ends the run, since carrying on would
/// silently drop messages.
pub async fn run(
  config: Config,
  shutdown: impl Future<Output = ()>,
) -> io::Result<Exit> {
  let Config {
    mut channel,
    list,
    reread_every,
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
  let control = control::dir(&journal_root);
  let mut writer = Writer::capture(journal_root, instance.clone())?;
  let mut client = connect(&chat, &channel);
  // The client's connections are numbered from 1; a new client's go on
  // from the last's.
  let (mut before, mut last) = (0, 0);
  let mut sync = ::tokio::time::interval(SYNC_INTERVAL);
  let mut heartbeat = ::tokio::time::interval(handoff.interval);
  let mut reread_at = interval_at(Instant::now() + reread_every, reread_every);
  let mut connected = None;
  let mut health = Health::default();
  let mut older = control::Older::default();
  let pusher = peer.clone().map(|peer| {
    let (control, name) = (control.clone(), channel.name.clone());
    let interval = handoff.interval;
    ::tokio::spawn(push_status(peer, control, name, say.clone(), interval))
  });
  ::tokio::pin!(shutdown);
  let exit = loop {
    ::tokio::select! {
      item = client.next() => {
        let Some(item) = item else { break Exit::Shutdown };
        let now = now_ms();
        match item {
          Item::Connected { connection } => {
            let connection = before + connection;
            (connected, last) = (Some(connection), connection);
            health = Health::default();
            writer.write(now, connection, Body::Open(channel.url.clone()))?;
          }
          Item::Frame(frame) => {
            health.saw(frame.event(), now);
            let connection = before + frame.connection;
            writer.write(now, connection, Body::Frame(frame.into_raw()))?;
          }
          Item::Binary { connection, data } => {
            writer.write(now, before + connection, Body::Binary(data))?;
          }
          Item::Disconnected { connection, reason } => {
            let connection = before + connection;
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
          listening: Some(channel.url.clone()),
        };
        // Control files are advisory: a failed round (e.g. Windows refusing
        // a rename while another instance reads the file) is retried next
        // tick rather than stopping capture.
        match check_handoff(&control, &me, &handoff, &mut older, Variant::Dgg)
        {
          Ok((successor, said)) => {
            for (level, line) in said {
              say.at(level, line);
            }
            if let Some(successor) = successor {
              break Exit::HandedOff(successor);
            }
          }
          Err(e) => say.warn(format_args!("control: {e}")),
        }
      }
      _ = reread_at.tick(), if list.is_some() => {
        let list = list.as_deref().unwrap();
        match reread(list, &channel, &say) {
          None => {}
          Some(Change::Removed) => {
            say.info(format_args!("{}: {REMOVED}; stopping", channel.name));
            break Exit::Removed;
          }
          Some(Change::Changed(now)) => {
            say.info(format_args!(
              "{}: {CHANGED}; connecting again",
              channel.name
            ));
            if let Some(connection) = connected.take() {
              let close = Body::Close(CHANGED.into());
              writer.write(now_ms(), connection, close)?;
            }
            channel = now;
            client = connect(&chat, &channel);
            before = last;
            health = Health::default();
          }
        }
      }
      () = &mut shutdown => break Exit::Shutdown,
    }
  };
  if let Some(connection) = connected {
    let reason = match &exit {
      Exit::Shutdown => SHUTDOWN.to_owned(),
      Exit::HandedOff(successor) => format!("{HANDOFF}{successor}"),
      Exit::Removed => REMOVED.to_owned(),
    };
    writer.write(now_ms(), connection, Body::Close(reason))?;
  }
  writer.sync()?;
  if let Err(e) = control::remove(&control, &instance) {
    say.warn(format_args!("control: {e}"));
  }
  if let (Some(pusher), Some(peer)) = (pusher, peer) {
    pusher.abort();
    withdraw_status(&peer, &channel.name, &say).await;
  }
  Ok(exit)
}

fn peer_client(peer: &Peer) -> Result<::reqwest::Client, String> {
  peer.client(::reqwest::Client::builder().timeout(Duration::from_secs(5)))
}

/// Where `instance`'s status goes on `peer`: its channel's, `channel`.
fn peer_url(peer: &Peer, channel: &str, instance: &str) -> String {
  let base = peer.url.trim_end_matches('/');
  let at = routes::<Dgg>();
  format!("{base}{at}/channels/{channel}/control/{instance}")
}

/// Pushes this instance's status file to `peer`'s `channel` every
/// `interval`, until aborted. Failures are reported when they start and
/// stop, not every round: the peer's copy just goes stale, so its instances
/// keep running.
async fn push_status(
  peer: Peer,
  control: PathBuf,
  channel: String,
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
      .put(peer_url(&peer, &channel, instance))
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

/// Removes this instance's status from `peer`'s `channel`, best effort.
async fn withdraw_status(peer: &Peer, channel: &str, say: &Say) {
  let sent = match peer_client(peer) {
    Ok(client) => client
      .delete(peer_url(peer, channel, &say.instance))
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
