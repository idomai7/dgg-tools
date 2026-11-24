//! Live chat from the logger's `serve`, which captures Kick's: each
//! channel's `GET /kick/v0/channels/<slug>/events`, server-sent events from
//! when the stream starts, with nothing replayed.
//!
//! - `event`: one of Kick's, decoded as kick-mod's own connection's are.
//! - `open` and `close`: the capture's connections, as the column's status:
//!   live while one's open. One opened before the stream started isn't
//!   known, so a stream starts live, and a close leaves it down until an
//!   open.
//! - `history`: a page of Kick's chat history that the capture filled a gap
//!   with, as [`What::Filled`]: its messages since the stream started, but
//!   those already seen.
//!
//! A stream that drops, or can't start, is retried, after
//! [`FIRST_RETRY`](crate::logger::FIRST_RETRY) doubling to [`LAST_RETRY`]; what's sent meanwhile is missing, which the
//! status says. One refused with `400`, `401` or `403` isn't: the config
//! needs fixing.

use {
  super::{Fill, Hub, MARGIN, Seen, State, What, filled_in},
  crate::logger::Client,
  ::common::{format::duration, time::now_ms},
  ::kick_api::{
    realtime::Event,
    web::{self, Back},
  },
  ::reqwest::StatusCode,
  ::serde::Deserialize,
  ::std::{collections::HashSet, sync::Arc, time::Duration},
  ::tokio::time::{sleep, timeout},
};

pub const LAST_RETRY: Duration = Duration::from_secs(30);

/// How long a stream may be silent, keep-alives included, before it's
/// taken as dropped: the logger sends one every 15 s.
const SILENCE: Duration = Duration::from_secs(45);

/// How a stream ended.
enum End {
  /// To be retried.
  Dropped(String),
  /// Not to be.
  Refused(String),
}

/// Follows channel `slug` through `logger` for good, or until it's refused,
/// publishing what happens in `hub`.
pub(super) async fn follow(slug: String, logger: Client, hub: Arc<Hub>) {
  let mut following = Following {
    slug: &slug,
    hub: &hub,
    since: now_ms() - MARGIN,
    seen: Seen::default(),
    open: HashSet::new(),
    shown: Some((State::Connecting, None)),
  };
  let mut retry = logger.first_retry;
  loop {
    let (up, end) = following.stream(&logger).await;
    if up {
      retry = logger.first_retry;
    }
    let why = match end {
      End::Refused(why) => {
        following.status(State::Stopped, Some(format!("the logger: {why}")));
        return;
      }
      End::Dropped(why) => why,
    };
    let again = duration(retry.as_millis() as i64);
    let detail = match up {
      true => format!(
        "the logger: {why}; chat meanwhile is missing; again in {again}"
      ),
      false => format!("the logger: {why}; again in {again}"),
    };
    following.status(State::Reconnecting, Some(detail));
    sleep(retry).await;
    retry = (retry * 2).min(LAST_RETRY);
  }
}

/// A channel's streams' state.
struct Following<'a> {
  slug: &'a str,
  hub: &'a Hub,
  /// When (unix ms) following started, margin included: a gap's messages
  /// from before are left out.
  since: i64,
  seen: Seen,
  /// The capture's connections known to be open, by instance.
  open: HashSet<(String, u64)>,
  /// The status the page was last told.
  shown: Option<(State, Option<String>)>,
}

/// An event's data, as the logger sends it.
#[derive(Deserialize)]
struct Sent {
  instance: String,
  connection: u64,
  #[serde(default)]
  reason: String,
  #[serde(default)]
  event: String,
  #[serde(default)]
  data: String,
  #[serde(default)]
  page: String,
}

impl Following<'_> {
  /// Streams once, until it ends: whether it started, and how it ended.
  async fn stream(&mut self, logger: &Client) -> (bool, End) {
    let request = logger.events(self.slug);
    let mut response = match timeout(SILENCE, request.send()).await {
      Err(_) => return (false, End::Dropped("no answer".into())),
      Ok(Err(e)) => return (false, End::Dropped(e.to_string())),
      Ok(Ok(response)) => response,
    };
    let status = response.status();
    if !status.is_success() {
      let why = format!("answered {status}");
      return match status {
        StatusCode::BAD_REQUEST
        | StatusCode::UNAUTHORIZED
        | StatusCode::FORBIDDEN => (false, End::Refused(why)),
        _ => (false, End::Dropped(why)),
      };
    }
    self.open.clear();
    self.status(State::Live, None);
    let mut events = Events::default();
    loop {
      let chunk = match timeout(SILENCE, response.chunk()).await {
        Err(_) => {
          let silent = duration(SILENCE.as_millis() as i64);
          return (true, End::Dropped(format!("silent for {silent}")));
        }
        Ok(Err(e)) => return (true, End::Dropped(e.to_string())),
        Ok(Ok(None)) => return (true, End::Dropped("it ended".into())),
        Ok(Ok(Some(chunk))) => chunk,
      };
      for (kind, data) in events.push(&chunk) {
        if let Some(end) = self.handle(&kind, &data) {
          return (true, end);
        }
      }
    }
  }

  /// Takes the logger's event `kind` with `data`: how the stream ends, if
  /// it does.
  fn handle(&mut self, kind: &str, data: &str) -> Option<End> {
    if kind == "error" {
      return Some(End::Dropped(format!("its error: {data}")));
    }
    let sent: Sent = match ::serde_json::from_str(data) {
      Ok(sent) => sent,
      Err(e) => {
        eprintln!("{}: the logger's {kind} event: {e}", self.slug);
        return None;
      }
    };
    match kind {
      "open" => {
        self.open.insert((sent.instance, sent.connection));
        self.status(State::Live, None);
      }
      "close" => {
        self.open.remove(&(sent.instance, sent.connection));
        if self.open.is_empty() {
          let detail = format!("the capture: {}", sent.reason);
          self.status(State::Reconnecting, Some(detail));
        }
      }
      "event" => {
        let event = Event::parse(&sent.event, &sent.data);
        if let Some(what) = What::from_event(event) {
          if let What::Message { id, .. } = &what
            && !self.seen.insert(id)
          {
            return None;
          }
          self.hub.publish(self.slug, what);
        }
      }
      "history" => match web::parse(&sent.page) {
        Ok(page) => {
          let fill = Fill {
            connection: sent.connection,
            since: self.since,
            back: Back {
              messages: page.messages,
              complete: true,
              ..Back::default()
            },
          };
          let filled = filled_in(fill, &mut self.seen);
          if let What::Filled { messages, .. } = &filled
            && !messages.is_empty()
          {
            self.hub.publish(self.slug, filled);
          }
        }
        Err(e) => eprintln!("{}: the logger's history page: {e}", self.slug),
      },
      // From a newer logger.
      _ => {}
    }
    None
  }

  /// Tells the page the channel's status, if it changed.
  fn status(&mut self, state: State, detail: Option<String>) {
    let status = (state, detail);
    if self.shown.as_ref() != Some(&status) {
      self.shown = Some(status.clone());
      self.hub.status(self.slug, status.0, status.1);
    }
  }
}

/// Server-sent events, from the chunks they arrive in.
#[derive(Default)]
struct Events {
  buffer: Vec<u8>,
}

impl Events {
  /// Adds `chunk`: each event it completes, its kind and data. Keep-alives,
  /// which have no data, are none.
  fn push(&mut self, chunk: &[u8]) -> Vec<(String, String)> {
    self.buffer.extend_from_slice(chunk);
    let mut events = Vec::new();
    while let Some(end) = self.buffer.windows(2).position(|w| w == b"\n\n") {
      let block: Vec<u8> = self.buffer.drain(..end + 2).collect();
      let block = String::from_utf8_lossy(&block);
      let mut kind = "message".to_owned();
      let mut data = Vec::new();
      for line in block.lines() {
        if let Some(value) = field(line, "event") {
          kind = value.into();
        } else if let Some(value) = field(line, "data") {
          data.push(value);
        }
      }
      if !data.is_empty() {
        events.push((kind, data.join("\n")));
      }
    }
    events
  }
}

/// The value of `line` if it's field `name`'s.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
  let value = line.strip_prefix(name)?.strip_prefix(':')?;
  Some(value.strip_prefix(' ').unwrap_or(value))
}
