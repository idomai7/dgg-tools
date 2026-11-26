//! Capture's machinery, shared by dgg's ([`crate::dgg::capture`]) and
//! Kick's ([`crate::kick::capture`]): instance ids, handoff, logging and
//! peers.
//!
//! Instances hand off to each other through [`control`](crate::control)
//! status files: an instance retires once a later-started one has been
//! healthy for the handoff grace period, so upgrading means starting the
//! new binary. An instance on another host takes over by pushing its status
//! to that host's `serve` ([`Peer`]).

use {
  crate::control::{self, Status},
  ::common::{
    log::Level,
    time::{now_ms, utc_timestamp},
  },
  ::logger_journal::Variant,
  ::std::{fmt::Display, io, path::Path, time::Duration},
};

/// The `close` reason written when capture is told to stop.
pub const SHUTDOWN: &str = "shutdown";

/// Prefix of the `close` reason written when capture retires,
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

/// Writes a capture's own lines to stderr, each at its [`Level`] and naming
/// the instance. A bridge's (a capture an install starts, with no supervisor
/// to timestamp its lines) are timestamped and marked too.
#[derive(Clone, Debug)]
pub struct Say {
  pub instance: String,
  pub bridge: bool,
}

impl Say {
  pub fn info(&self, message: impl Display) {
    self.at(Level::Info, message);
  }

  pub fn warn(&self, message: impl Display) {
    self.at(Level::Warn, message);
  }

  pub fn error(&self, message: impl Display) {
    self.at(Level::Error, message);
  }

  pub fn at(&self, level: Level, message: impl Display) {
    if self.bridge {
      eprintln!(
        "{} {level} {} (bridge): {message}",
        utc_timestamp(now_ms()),
        self.instance
      );
    } else {
      eprintln!("{level} {}: {message}", self.instance);
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

/// Another host's `serve`.
#[derive(Clone, Debug, Default)]
pub struct Peer {
  /// e.g. `https://203.0.113.5:8788`.
  pub url: String,
  pub token: String,
  /// The mesh CA's certificate (PEM): if given, the peer is reached over
  /// HTTPS only, trusting that CA alone (see [`crate::cert`]).
  pub ca: Option<String>,
}

impl Peer {
  /// A client for this peer: trusting its CA alone, if it has one.
  pub fn client(
    &self,
    builder: ::reqwest::ClientBuilder,
  ) -> Result<::reqwest::Client, String> {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let builder = match &self.ca {
      Some(ca) => crate::cert::trusting(builder, ca)?,
      None => builder,
    };
    builder
      .build()
      .map_err(|e| format!("can't make a client: {e}"))
  }
}

/// Why a capture returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
  /// `shutdown` completed.
  Shutdown,
  /// Retired in favour of this instance.
  HandedOff(String),
  /// Its channel was removed from the list.
  Removed,
}

/// `dgg-capture`'s exit code when its channel was removed from the list:
/// not 0, which tells the supervisor it handed off (an upgrade's under
/// way), nor 1 (failed) or 2 (clap's, for bad arguments).
pub const REMOVED_EXIT: u8 = 3;

/// Writes `me`, an instance of `variant`'s capture, into the control
/// directory `control`, sweeps away the statuses there left by instances
/// that stopped without removing their own, then returns the successor to
/// hand off to, if there is one and handing off is allowed, and what it
/// has to say.
pub(crate) fn check_handoff(
  control: &Path,
  me: &Status,
  handoff: &Handoff,
  older: &mut control::Older,
  variant: Variant,
) -> io::Result<(Option<String>, control::Said)> {
  control::write_as(control, me, variant)?;
  let others = control::read_all_as(control, variant)?;
  let mut said: control::Said = older
    .gone(me, &others)
    .into_iter()
    .map(|line| (Level::Info, line))
    .collect();
  let swept = control::sweepable(
    me,
    &others,
    me.heartbeat,
    handoff.stale_after,
    control::SWEEP_AFTER,
  );
  for status in swept {
    said.push((
      Level::Warn,
      match control::remove(control, &status.instance) {
        Ok(()) => control::swept_line(status, me.heartbeat),
        Err(e) => format!("can't remove {}'s status: {e}", status.instance),
      },
    ));
    older.forget(&status.instance);
  }
  if handoff.keep_running {
    return Ok((None, said));
  }
  let successor = control::successor(
    me,
    &others,
    me.heartbeat,
    handoff.grace,
    handoff.stale_after,
  );
  Ok((successor.map(|status| status.instance.clone()), said))
}
