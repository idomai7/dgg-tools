//! Kick capture: each channel's chat into the journal, raw, and nothing
//! else.
//!
//! Each channel in the [list](super::channels) gets a realtime connection of
//! its own, subscribed to `chatrooms.<chatroom id>.v2`, so one channel's
//! connection failing leaves the others. Each is journaled in its channel's
//! journal root, `<journal>/kick/<slug>/`, in Kick's format. A session Kick
//! stops for good is started again after [`Config::restart_after`].
//!
//! Kick keeps no history on the connection, so when one comes back after a
//! drop, the gap's messages are fetched from Kick's chat history
//! ([`Web::back_to`]), back to a little before the last thing heard, at most
//! [`MAX_PAGES`] pages. The pages are journaled raw, on the connection that
//! came back. Only messages come back: deletions, bans and the rest in the
//! gap stay missing.
//!
//! Instances hand off as dgg's capture's do (see [`crate::capture`]), through
//! a `capture-status kick.0.0` file in each channel's `control/`. An
//! instance is healthy once every one of its channels is subscribed, and
//! retires once a later-started one has been healthy for the grace period in
//! any of its channels, so a new instance's list wins.

use {
  super::channels::Channel,
  crate::{
    capture::{Exit, HANDOFF, Handoff, SHUTDOWN, Say},
    control::{self, Status},
  },
  ::common::log::Level,
  ::common::time::now_ms,
  ::kick_api::{
    realtime::{Client, Item, Options},
    web::{Back, Web},
  },
  ::logger_journal::{
    Variant,
    layout::channel_root,
    record::kick::v0::Body,
    writer::{SYNC_INTERVAL, kick::Writer},
  },
  ::std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
  },
  ::tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::{interval, sleep},
  },
};

/// The most pages of history a gap is filled from: 250 messages.
pub const MAX_PAGES: usize = 10;

/// How long before the last thing heard a gap is filled from (ms): Kick's
/// times are whole seconds, and its clock isn't ours.
const MARGIN: i64 = 5_000;

pub struct Config {
  /// Each captured, with its own connection.
  pub channels: Vec<Channel>,
  /// Every connection's options, but for its channels.
  pub realtime: Options,
  /// Where gaps are filled from; with none, they aren't.
  pub web: Option<Arc<Web>>,
  /// Kick's journal root (see
  /// [`kick_root`](::logger_journal::layout::kick_root)).
  pub kick_root: PathBuf,
  pub instance: String,
  /// When this instance started, unix ms. Later-started instances win.
  pub started: i64,
  pub handoff: Handoff,
  /// How long a channel waits to start again after Kick stops its session.
  pub restart_after: Duration,
  /// Whether this is a bridge, for its log lines (see [`Say`]).
  pub bridge: bool,
}

/// Streams every channel's chat into its journal until `shutdown`
/// completes or a successor takes over, then closes each open connection's
/// record, syncs, and removes this instance's status files.
///
/// A journal write error ends the run, every channel's, since carrying on
/// would silently drop messages.
pub async fn run(
  config: Config,
  shutdown: impl Future<Output = ()>,
) -> io::Result<Exit> {
  let Config {
    channels,
    realtime,
    web,
    kick_root,
    instance,
    started,
    handoff,
    restart_after,
    bridge,
  } = config;
  let say = Say {
    instance: instance.clone(),
    bridge,
  };
  let (stop, stopped) = watch::channel(None);
  let mut tasks = JoinSet::new();
  let mut subscribed = Vec::new();
  let mut controls = Vec::new();
  for channel in channels {
    let root = channel_root(&kick_root, &channel.slug);
    controls.push(control::dir(&root));
    let (since, since_rx) = watch::channel(None);
    subscribed.push(since_rx);
    let listening = Listening {
      options: Options {
        channels: vec![format!("chatrooms.{}.v2", channel.chatroom_id)],
        ..realtime.clone()
      },
      writer: Writer::capture(root, &instance, &channel.slug)?,
      filler: web.clone().map(|web| (web, channel.channel_id)),
      restart_after,
      subscribed: since,
      stop: stopped.clone(),
      say: say.clone(),
      slug: channel.slug,
    };
    tasks.spawn(listening.run());
  }
  let mut heartbeat = interval(handoff.interval);
  let mut older = control::Older::default();
  ::tokio::pin!(shutdown);
  let ended = loop {
    ::tokio::select! {
      _ = heartbeat.tick() => {
        // Healthy once the last channel subscribed, while all still are.
        let healthy_since = subscribed
          .iter()
          .map(|since| *since.borrow())
          .collect::<Option<Vec<i64>>>()
          .and_then(|since| since.into_iter().max());
        let me = Status {
          instance: instance.clone(),
          started,
          heartbeat: now_ms(),
          healthy_since,
          build: Some(crate::BUILD.into()),
        };
        // Advisory, as dgg's: a failed round is retried next tick.
        match check_handoff(&controls, &me, &handoff, &mut older) {
          Ok((successor, gone)) => {
            for line in gone {
              say.info(line);
            }
            if let Some(successor) = successor {
              break Ok(Exit::HandedOff(successor));
            }
          }
          Err(e) => say.warn(format_args!("control: {e}")),
        }
      }
      Some(ended) = tasks.join_next() => {
        break Err(match ended {
          Ok(Err(e)) => e,
          Ok(Ok(())) => io::Error::other("a channel stopped"),
          Err(e) => io::Error::other(e),
        });
      }
      () = &mut shutdown => break Ok(Exit::Shutdown),
    }
  };
  let reason = match &ended {
    Ok(Exit::Shutdown) => SHUTDOWN.to_owned(),
    Ok(Exit::HandedOff(successor)) => format!("{HANDOFF}{successor}"),
    Err(e) => format!("failed: {e}"),
  };
  stop.send_replace(Some(reason));
  let mut result = ended;
  while let Some(ended) = tasks.join_next().await {
    let error = match ended {
      Ok(Ok(())) => continue,
      Ok(Err(e)) => e,
      Err(e) => io::Error::other(e),
    };
    if result.is_ok() {
      result = Err(error);
    }
  }
  for dir in &controls {
    if let Err(e) = control::remove(dir, &instance) {
      say.warn(format_args!("control: {e}"));
    }
  }
  result
}

/// Writes this instance's status in each channel's control directory,
/// then returns the successor to hand off to, if there's one in any and
/// handing off is allowed.
fn check_handoff(
  controls: &[PathBuf],
  me: &Status,
  handoff: &Handoff,
  older: &mut control::Older,
) -> io::Result<(Option<String>, Vec<String>)> {
  for dir in controls {
    control::write_as(dir, me, Variant::Kick)?;
  }
  let mut others = Vec::new();
  for dir in controls {
    others.extend(control::read_all_as(dir, Variant::Kick)?);
  }
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

/// One channel's capture.
struct Listening {
  slug: String,
  options: Options,
  writer: Writer,
  /// Where its gaps are filled from, and its channel id.
  filler: Option<(Arc<Web>, u64)>,
  restart_after: Duration,
  /// Since when (unix ms) it's been subscribed, if it is.
  subscribed: watch::Sender<Option<i64>>,
  /// Set to the reason, to close and stop.
  stop: watch::Receiver<Option<String>>,
  say: Say,
}

/// What a gap's fill fetched.
struct Fill {
  /// The connection that came back.
  connection: u64,
  back: Back,
}

/// A connection's state between items.
#[derive(Default)]
struct State {
  /// Added to the client's connection numbers: a restarted client counts
  /// from 1 again, and an instance's connections are numbered once.
  offset: u64,
  /// The connection that's open, for a stop to close.
  open: Option<u64>,
  /// The connection last opened, for a fill to be journaled on.
  connection: Option<u64>,
  /// When (unix ms) something was last heard while subscribed.
  heard: Option<i64>,
  /// When something was last heard before a drop, until it's filled.
  gap: Option<i64>,
}

impl Listening {
  async fn run(mut self) -> io::Result<()> {
    let mut client = Client::spawn(self.options.clone());
    let mut sync = interval(SYNC_INTERVAL);
    let mut state = State::default();
    let (filled, mut fills) = mpsc::unbounded_channel::<Fill>();
    loop {
      let item = ::tokio::select! {
        item = client.next() => item,
        Some(fill) = fills.recv() => {
          self.journal_fill(fill)?;
          continue;
        }
        _ = sync.tick() => {
          self.writer.sync()?;
          continue;
        }
        _ = self.stop.changed() => break,
      };
      let Some(item) = item else {
        // Kick stopped the session: start again, later.
        state.offset = state.connection.unwrap_or(state.offset);
        let wait = sleep(self.restart_after);
        ::tokio::select! {
          () = wait => {}
          _ = self.stop.changed() => break,
        }
        client = Client::spawn(self.options.clone());
        continue;
      };
      self.take(item, &mut state, &filled)?;
    }
    if let Some(connection) = state.open {
      let reason = self.stop.borrow().clone().unwrap_or_default();
      self
        .writer
        .write(now_ms(), connection, Body::Close(reason))?;
    }
    self.writer.sync()
  }

  /// Journals `item`, and follows the connection's state.
  fn take(
    &mut self,
    item: Item,
    state: &mut State,
    filled: &mpsc::UnboundedSender<Fill>,
  ) -> io::Result<()> {
    let now = now_ms();
    let offset = state.offset;
    let (connection, body) = match item {
      Item::Connected {
        connection,
        endpoint,
      } => {
        let connection = connection + offset;
        state.open = Some(connection);
        state.connection = Some(connection);
        (connection, Body::Open(endpoint))
      }
      Item::Frame(frame) => {
        state.heard = Some(now);
        (frame.connection + offset, Body::Frame(frame.into_raw()))
      }
      Item::Binary { connection, data } => {
        (connection + offset, Body::Binary(data))
      }
      Item::Disconnected {
        connection,
        code,
        reason,
      } => {
        self.dropped(state);
        state.open = None;
        let code = code.map(|code| format!("{code} ")).unwrap_or_default();
        let reason = format!("{code}{reason}");
        self.say(
          Level::Warn,
          format_args!("disconnected (#{connection}): {reason}"),
        );
        (connection + offset, Body::Close(reason))
      }
      // Ends the open connection, with no disconnect of its own.
      Item::Stopped { code, reason } => {
        self.dropped(state);
        let reason = format!("{code} {reason}");
        let again =
          ::common::format::duration(self.restart_after.as_millis() as i64);
        self.say(
          Level::Warn,
          format_args!("stopped: {reason}; again in {again}"),
        );
        let Some(connection) = state.open.take() else {
          return Ok(());
        };
        (connection, Body::Close(reason))
      }
      Item::ConnectFailed {
        stage,
        error,
        retry_in,
      } => {
        self.say(
          Level::Warn,
          format_args!(
            "connect failed: {stage}: {error}; retrying in {retry_in:?}"
          ),
        );
        return Ok(());
      }
      Item::Unsubscribed {
        code,
        reason,
        resubscribing,
        ..
      } => {
        self.dropped(state);
        let again = if resubscribing {
          "resubscribing"
        } else {
          "not again"
        };
        self.say(
          Level::Warn,
          format_args!("unsubscribed: {code} {reason}; {again}"),
        );
        return Ok(());
      }
      Item::Subscribed { .. } => {
        if self.subscribed.borrow().is_none() {
          self.subscribed.send_replace(Some(now));
        }
        if let Some(heard) = state.gap.take()
          && let Some((web, channel_id)) = &self.filler
          && let Some(connection) = state.connection
        {
          let (web, channel_id) = (web.clone(), *channel_id);
          let filled = filled.clone();
          ::tokio::spawn(async move {
            let back = web.back_to(channel_id, heard - MARGIN, MAX_PAGES);
            let fill = Fill {
              connection,
              back: back.await,
            };
            // None waiting is fine: the channel's stopping.
            let _ = filled.send(fill);
          });
        }
        state.heard = Some(now);
        return Ok(());
      }
    };
    self.writer.write(now, connection, body)
  }

  /// Notes the connection or subscription dropped: a gap from the last
  /// thing heard.
  fn dropped(&self, state: &mut State) {
    self.subscribed.send_replace(None);
    state.gap = state.gap.or(state.heard.take());
  }

  /// Journals the pages a gap was filled from.
  fn journal_fill(&mut self, fill: Fill) -> io::Result<()> {
    let Fill { connection, back } = fill;
    let (pages, messages) = (back.pages.len(), back.messages.len());
    match &back.error {
      None if back.complete => self.say(
        Level::Info,
        format_args!("filled a gap: {messages} messages from {pages} pages"),
      ),
      None => self.say(
        Level::Warn,
        format_args!(
          "filled a gap in part: {messages} messages from {pages} pages, \
         the most fetched"
        ),
      ),
      Some(e) => self.say(
        Level::Warn,
        format_args!(
          "filled a gap in part: {messages} messages from {pages} pages; {e}"
        ),
      ),
    }
    let now = now_ms();
    for (cursor, page) in back.pages {
      self
        .writer
        .write(now, connection, Body::History { cursor, page })?;
    }
    Ok(())
  }

  fn say(&self, level: Level, message: impl ::std::fmt::Display) {
    self.say.at(level, format_args!("{}: {message}", self.slug));
  }
}

/// The control directory of `slug`'s journal root under `kick_root`.
pub fn control_dir(kick_root: &Path, slug: &str) -> PathBuf {
  control::dir(&channel_root(kick_root, slug))
}
