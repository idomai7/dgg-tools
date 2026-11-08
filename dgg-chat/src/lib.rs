//! Shared destiny.gg chat client.
//!
//! [`Client`] keeps a socket to dgg chat open, reconnecting with backoff, and
//! yields every text frame plus connect/disconnect markers so callers can see
//! gaps. `PING` frames are answered internally and not yielded; binary frames
//! are yielded as-is.

use {
  ::futures_util::{SinkExt, StreamExt},
  ::std::time::Duration,
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

pub const DEFAULT_URL: &str = "wss://chat.destiny.gg/ws";
pub const DEFAULT_ORIGIN: &str = "https://www.destiny.gg";

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
  pub conn: u64,
  raw: String,
  split: usize,
}

impl Frame {
  pub fn parse(conn: u64, raw: String) -> Self {
    let split = raw.find(' ').unwrap_or(raw.len());
    Self { conn, raw, split }
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
    conn: u64,
  },
  Frame(Frame),
  /// A binary frame. dgg doesn't send these today; they're passed through
  /// so a protocol change can't be silently dropped.
  Binary {
    conn: u64,
    data: Vec<u8>,
  },
  Disconnected {
    conn: u64,
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
  task: JoinHandle<()>,
}

impl Client {
  pub fn spawn(opts: Options) -> Self {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let (tx, rx) = mpsc::unbounded_channel();
    let task = ::tokio::spawn(run(opts, tx));
    Self { rx, task }
  }

  pub async fn next(&mut self) -> Option<Item> {
    self.rx.recv().await
  }
}

impl Drop for Client {
  fn drop(&mut self) {
    self.task.abort();
  }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Tx = mpsc::UnboundedSender<Item>;

async fn run(opts: Options, tx: Tx) {
  let mut backoff = opts.min_backoff;
  let mut conn = 0;
  loop {
    match connect(&opts).await {
      Ok(ws) => {
        conn += 1;
        if tx.send(Item::Connected { conn }).is_err() {
          return;
        }
        let started = Instant::now();
        let Some(reason) = pump(ws, conn, &opts, &tx).await else {
          return;
        };
        if tx.send(Item::Disconnected { conn, reason }).is_err() {
          return;
        }
        if started.elapsed() >= STABLE_AFTER {
          backoff = opts.min_backoff;
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
    backoff = (backoff * 2).min(opts.max_backoff);
  }
}

async fn connect(opts: &Options) -> Result<Ws, String> {
  let mut req = opts
    .url
    .as_str()
    .into_client_request()
    .map_err(|e| e.to_string())?;
  let headers = req.headers_mut();
  for (name, value) in [("Origin", &opts.origin), ("Cookie", &opts.cookie)] {
    if let Some(value) = value {
      let value = HeaderValue::from_str(value).map_err(|e| e.to_string())?;
      headers.insert(name, value);
    }
  }
  match timeout(opts.idle_timeout, ::tokio_tungstenite::connect_async(req))
    .await
  {
    Ok(Ok((ws, _))) => Ok(ws),
    Ok(Err(e)) => Err(e.to_string()),
    Err(_) => Err("connect timed out".into()),
  }
}

/// Forwards frames until the socket drops. Returns the reason, or `None` if
/// the receiver has gone away.
async fn pump(
  mut ws: Ws,
  conn: u64,
  opts: &Options,
  tx: &Tx,
) -> Option<String> {
  let mut ping = ::tokio::time::interval(opts.idle_timeout / 3);
  ping.tick().await;
  let mut last = Instant::now();
  loop {
    ::tokio::select! {
      msg = ws.next() => {
        last = Instant::now();
        match msg {
          Some(Ok(Message::Text(text))) => {
            let frame = Frame::parse(conn, text.as_str().to_owned());
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
            let item = Item::Binary { conn, data: data.to_vec() };
            if tx.send(item).is_err() {
              return None;
            }
          }
          Some(Ok(Message::Close(Some(f)))) => {
            return Some(format!("closed: {} {}", u16::from(f.code), f.reason));
          }
          Some(Ok(Message::Close(None))) => return Some("closed".into()),
          Some(Ok(_)) => {}
          Some(Err(e)) => return Some(e.to_string()),
          None => return Some("stream ended".into()),
        }
      }
      _ = ping.tick() => {
        if last.elapsed() >= opts.idle_timeout {
          return Some("idle timeout".into());
        }
        if let Err(e) = ws.send(Message::Ping(Default::default())).await {
          return Some(format!("send failed: {e}"));
        }
      }
    }
  }
}
