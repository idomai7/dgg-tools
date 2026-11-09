//! The chat socket client.
//!
//! [`Client`] keeps a socket to dgg chat open, reconnecting with backoff, and
//! yields every text frame plus connect/disconnect markers so callers can see
//! gaps. `PING` frames are answered internally and not yielded; binary frames
//! are yielded as-is.
//!
//! [`Client::send`] sends frames as given, raw: encoding commands is
//! `protocol`'s job. Sending needs a logged-in connection ([`Options`]'
//! cookie) for most commands.

use {
  super::{DEFAULT_ORIGIN, DEFAULT_URL},
  ::futures_util::{SinkExt, StreamExt},
  ::std::{
    error, fmt,
    sync::{
      Arc,
      atomic::{AtomicU64, Ordering},
    },
    time::Duration,
  },
  ::tokio::{
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
  },
  ::tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
  },
};

/// A connection that stayed up this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct Options {
  pub url: String,
  pub origin: Option<String>,
  /// Raw `Cookie` header value, for authenticated connections.
  pub cookie: Option<String>,
  pub min_backoff: Duration,
  pub max_backoff: Duration,
  /// Reconnect if nothing arrives for this long. Websocket pings are sent
  /// at a third of this interval to keep a quiet socket alive.
  pub idle_timeout: Duration,
}

impl Default for Options {
  fn default() -> Self {
    Self {
      url: DEFAULT_URL.into(),
      origin: Some(DEFAULT_ORIGIN.into()),
      cookie: None,
      min_backoff: Duration::from_secs(1),
      max_backoff: Duration::from_secs(60),
      idle_timeout: Duration::from_secs(90),
    }
  }
}

/// One text frame, `EVENT payload`, split on the first space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
  /// Which connection (counting from 1) the frame arrived on.
  pub connection: u64,
  raw: String,
  split: usize,
}

impl Frame {
  pub fn parse(connection: u64, raw: String) -> Self {
    let split = raw.find(' ').unwrap_or(raw.len());
    Self {
      connection,
      raw,
      split,
    }
  }

  pub fn event(&self) -> &str {
    &self.raw[..self.split]
  }

  pub fn payload(&self) -> &str {
    self.raw.get(self.split + 1..).unwrap_or("")
  }

  /// The frame exactly as received.
  pub fn raw(&self) -> &str {
    &self.raw
  }

  pub fn into_raw(self) -> String {
    self.raw
  }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
  Connected {
    connection: u64,
  },
  Frame(Frame),
  /// A binary frame. dgg doesn't send these today; they're passed through
  /// so a protocol change can't be silently dropped.
  Binary {
    connection: u64,
    data: Vec<u8>,
  },
  Disconnected {
    connection: u64,
    reason: String,
  },
  ConnectFailed {
    error: String,
    retry_in: Duration,
  },
}

/// A self-reconnecting dgg chat connection. Dropping it closes the socket.
pub struct Client {
  rx: mpsc::UnboundedReceiver<Item>,
  outgoing: mpsc::UnboundedSender<String>,
  /// The open connection's number; 0 while there's none.
  connected: Arc<AtomicU64>,
  task: JoinHandle<()>,
}

/// [`Client::send`] found no open connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotConnected;

impl fmt::Display for NotConnected {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.write_str("not connected to chat")
  }
}

impl error::Error for NotConnected {}

impl Client {
  pub fn spawn(options: Options) -> Self {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let (tx, rx) = mpsc::unbounded_channel();
    let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
    let connected = Arc::new(AtomicU64::new(0));
    let task =
      ::tokio::spawn(run(options, tx, outgoing_rx, Arc::clone(&connected)));
    Self {
      rx,
      outgoing,
      connected,
      task,
    }
  }

  pub async fn next(&mut self) -> Option<Item> {
    self.rx.recv().await
  }

  /// Sends `frame` (`EVENT payload`) on the open connection, returning its
  /// number. Frames aren't kept for a later connection: one sent as the
  /// connection drops is discarded, so a reconnect never replays stale
  /// commands. Chat confirms nothing directly; what a command did comes
  /// back as events (or an `ERR`).
  pub fn send(&self, frame: impl Into<String>) -> Result<u64, NotConnected> {
    match self.connected.load(Ordering::Acquire) {
      0 => Err(NotConnected),
      connection => {
        self.outgoing.send(frame.into()).map_err(|_| NotConnected)?;
        Ok(connection)
      }
    }
  }
}

impl Drop for Client {
  fn drop(&mut self) {
    self.task.abort();
  }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Tx = mpsc::UnboundedSender<Item>;
type Outgoing = mpsc::UnboundedReceiver<String>;

async fn run(
  options: Options,
  tx: Tx,
  mut outgoing: Outgoing,
  connected: Arc<AtomicU64>,
) {
  let mut backoff = options.min_backoff;
  let mut connection = 0;
  loop {
    match connect(&options).await {
      Ok(ws) => {
        connection += 1;
        // Frames sent as the last connection dropped are stale now.
        while outgoing.try_recv().is_ok() {}
        connected.store(connection, Ordering::Release);
        if tx.send(Item::Connected { connection }).is_err() {
          return;
        }
        let started = Instant::now();
        let reason = pump(ws, connection, &options, &tx, &mut outgoing).await;
        connected.store(0, Ordering::Release);
        let Some(reason) = reason else {
          return;
        };
        if tx.send(Item::Disconnected { connection, reason }).is_err() {
          return;
        }
        if started.elapsed() >= STABLE_AFTER {
          backoff = options.min_backoff;
        }
      }
      Err(error) => {
        let item = Item::ConnectFailed {
          error,
          retry_in: backoff,
        };
        if tx.send(item).is_err() {
          return;
        }
      }
    }
    sleep(backoff).await;
    backoff = (backoff * 2).min(options.max_backoff);
  }
}

async fn connect(options: &Options) -> Result<Ws, String> {
  let mut request = options
    .url
    .as_str()
    .into_client_request()
    .map_err(|e| e.to_string())?;
  let headers = request.headers_mut();
  for (name, value) in
    [("Origin", &options.origin), ("Cookie", &options.cookie)]
  {
    if let Some(value) = value {
      let value = HeaderValue::from_str(value).map_err(|e| e.to_string())?;
      headers.insert(name, value);
    }
  }
  match timeout(
    options.idle_timeout,
    ::tokio_tungstenite::connect_async(request),
  )
  .await
  {
    Ok(Ok((ws, _))) => Ok(ws),
    Ok(Err(e)) => Err(e.to_string()),
    Err(_) => Err("connect timed out".into()),
  }
}

/// Forwards frames until the socket drops, and sends what's sent. Returns
/// the reason, or `None` if the receiver has gone away.
async fn pump(
  mut ws: Ws,
  connection: u64,
  options: &Options,
  tx: &Tx,
  outgoing: &mut Outgoing,
) -> Option<String> {
  let mut ping = ::tokio::time::interval(options.idle_timeout / 3);
  ping.tick().await;
  let mut last = Instant::now();
  loop {
    ::tokio::select! {
      message = ws.next() => {
        last = Instant::now();
        match message {
          Some(Ok(Message::Text(text))) => {
            let frame = Frame::parse(connection, text.as_str().to_owned());
            if frame.event() == "PING" {
              let pong = Message::text(format!("PONG {}", frame.payload()));
              if let Err(e) = ws.send(pong).await {
                return Some(format!("send failed: {e}"));
              }
            } else if tx.send(Item::Frame(frame)).is_err() {
              return None;
            }
          }
          Some(Ok(Message::Binary(data))) => {
            let item = Item::Binary { connection, data: data.to_vec() };
            if tx.send(item).is_err() {
              return None;
            }
          }
          Some(Ok(Message::Close(Some(close)))) => {
            let code = u16::from(close.code);
            return Some(format!("closed: {code} {}", close.reason));
          }
          Some(Ok(Message::Close(None))) => return Some("closed".into()),
          Some(Ok(_)) => {}
          Some(Err(e)) => return Some(e.to_string()),
          None => return Some("stream ended".into()),
        }
      }
      Some(frame) = outgoing.recv() => {
        if let Err(e) = ws.send(Message::text(frame)).await {
          return Some(format!("send failed: {e}"));
        }
      }
      _ = ping.tick() => {
        if last.elapsed() >= options.idle_timeout {
          return Some("idle timeout".into());
        }
        if let Err(e) = ws.send(Message::Ping(Default::default())).await {
          return Some(format!("send failed: {e}"));
        }
      }
    }
  }
}
