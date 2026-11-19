//! The realtime socket client.
//!
//! [`Client`] keeps a Centrifugo session with Kick open: it asks Kick's
//! [setup](super::connection) for a socket and a token, connects,
//! subscribes to its channels, refreshes the token before it expires, and
//! reconnects with backoff, resubscribing. It yields every text frame,
//! plus markers for the connection and each channel, so callers can see
//! gaps.
//!
//! Pings (`{}`) are answered internally and not yielded. Binary frames are
//! yielded as-is. Frames are decoded only to run the session; one that
//! doesn't decode is still yielded.
//!
//! A disconnect code of 3500 or up ends the stream with [`Item::Stopped`],
//! as centrifuge-js doesn't retry those either.

use {
  super::{
    centrifugo::{
      self, Command, Reply,
      command::{
        self, ConnectRequest, RefreshRequest, SubscribeRequest,
        UnsubscribeRequest,
      },
      push, reply,
    },
    connection,
  },
  ::futures_util::{SinkExt, StreamExt},
  ::std::{
    collections::{BTreeSet, HashMap},
    fmt,
    future::pending,
    time::Duration,
  },
  ::tokio::{
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout},
  },
  ::tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, tungstenite::Message,
  },
};

/// A connection that stayed up this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// Disconnect codes from here up aren't retried.
const TERMINAL: u32 = 3500;
/// Unsubscribe codes from here up are resubscribed.
const RESUBSCRIBE: u32 = 2500;
/// The connect command's id; the session's later commands count up from
/// the next.
const CONNECT_ID: u32 = 1;

/// Where the socket and its token come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
  /// Asked of Kick's setup at `base` for every connection, and the token
  /// refreshed before it expires.
  Kick { base: String },
  /// Given: connects to `endpoint` with `token` every time, and never
  /// refreshes it.
  Fixed { endpoint: String, token: String },
}

#[derive(Clone, Debug)]
pub struct Options {
  pub setup: Setup,
  /// The channels to subscribe to, such as `chatrooms.<chatroom id>.v2`.
  pub channels: Vec<String>,
  pub min_backoff: Duration,
  pub max_backoff: Duration,
  /// Reconnect if nothing arrives for this long. The server pings every
  /// 25 s.
  pub idle_timeout: Duration,
  /// How long each step of connecting may take: a setup request, opening
  /// the socket, the connect reply.
  pub connect_timeout: Duration,
}

impl Default for Options {
  fn default() -> Self {
    Self {
      setup: Setup::Kick {
        base: connection::DEFAULT_BASE.into(),
      },
      channels: Vec::new(),
      min_backoff: Duration::from_secs(1),
      max_backoff: Duration::from_secs(60),
      idle_timeout: Duration::from_secs(60),
      connect_timeout: Duration::from_secs(10),
    }
  }
}

/// One text frame, as received. It may hold several replies, one per line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
  /// Which connection (counting from 1) the frame arrived on.
  pub connection: u64,
  raw: String,
}

impl Frame {
  pub fn new(connection: u64, raw: String) -> Self {
    Self { connection, raw }
  }

  /// The frame exactly as received.
  pub fn raw(&self) -> &str {
    &self.raw
  }

  pub fn into_raw(self) -> String {
    self.raw
  }

  /// The frame's replies, decoded.
  pub fn replies(
    &self,
  ) -> impl Iterator<Item = Result<Reply, ::serde_json::Error>> + '_ {
    centrifugo::replies(&self.raw)
  }
}

/// Which step of connecting failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
  /// Kick's `connection` endpoint, asked which socket to open.
  Connection,
  /// Kick's `auth/connection` endpoint, asked for a token.
  Token,
  /// Opening the socket.
  Transport,
  /// Centrifugo's answer to the connect command.
  ConnectReply,
}

impl fmt::Display for Stage {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.write_str(match self {
      Self::Connection => "connection endpoint",
      Self::Token => "token",
      Self::Transport => "transport",
      Self::ConnectReply => "connect reply",
    })
  }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
  /// Centrifugo accepted the connect command, on the socket at `endpoint`.
  /// Its reply follows as the connection's first frame.
  Connected {
    connection: u64,
    endpoint: String,
  },
  Frame(Frame),
  /// A binary frame. A JSON client shouldn't get these; they're passed
  /// through so a protocol change can't be silently dropped.
  Binary {
    connection: u64,
    data: Vec<u8>,
  },
  /// The connection dropped, and will be retried. `code` is the server's
  /// disconnect code or the socket's close code, when there was one.
  Disconnected {
    connection: u64,
    code: Option<u32>,
    reason: String,
  },
  ConnectFailed {
    stage: Stage,
    error: String,
    retry_in: Duration,
  },
  Subscribed {
    connection: u64,
    channel: String,
  },
  /// The channel isn't subscribed any more: a refused subscribe, the server
  /// unsubscribing it, or [`Client::unsubscribe`] (code 0). From code 2500
  /// up it's resubscribed; below, it stays a gap until subscribed again.
  Unsubscribed {
    connection: u64,
    channel: String,
    code: u32,
    reason: String,
  },
  /// The server disconnected with a code that isn't retried. The stream
  /// ends after this.
  Stopped {
    code: u32,
    reason: String,
  },
}

enum Control {
  Subscribe(String),
  Unsubscribe(String),
}

/// A self-reconnecting Kick realtime connection. Dropping it closes the
/// socket.
pub struct Client {
  rx: mpsc::UnboundedReceiver<Item>,
  control: mpsc::UnboundedSender<Control>,
  task: JoinHandle<()>,
}

impl Client {
  pub fn spawn(options: Options) -> Self {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let (tx, rx) = mpsc::unbounded_channel();
    let (control, control_rx) = mpsc::unbounded_channel();
    let task = ::tokio::spawn(run(options, tx, control_rx));
    Self { rx, control, task }
  }

  pub async fn next(&mut self) -> Option<Item> {
    self.rx.recv().await
  }

  /// Adds `channel` to the subscriptions: now, if connected, and on every
  /// later connection.
  pub fn subscribe(&self, channel: impl Into<String>) {
    let _ = self.control.send(Control::Subscribe(channel.into()));
  }

  /// Drops `channel` from the subscriptions.
  pub fn unsubscribe(&self, channel: impl Into<String>) {
    let _ = self.control.send(Control::Unsubscribe(channel.into()));
  }
}

impl Drop for Client {
  fn drop(&mut self) {
    self.task.abort();
  }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Tx = mpsc::UnboundedSender<Item>;
type Controls = mpsc::UnboundedReceiver<Control>;

/// How a connection, or connecting, ended.
enum End {
  /// Retried after a backoff.
  Dropped { code: Option<u32>, reason: String },
  /// Not retried.
  Stopped { code: u32, reason: String },
  /// The receiver has gone away.
  Gone,
}

impl End {
  /// A disconnect or close code, and its reason.
  fn coded(code: u32, reason: String) -> Self {
    if code >= TERMINAL {
      Self::Stopped { code, reason }
    } else {
      Self::Dropped {
        code: Some(code),
        reason,
      }
    }
  }

  fn dropped(reason: impl Into<String>) -> Self {
    Self::Dropped {
      code: None,
      reason: reason.into(),
    }
  }
}

async fn run(options: Options, tx: Tx, mut controls: Controls) {
  let http = ::reqwest::Client::builder()
    .timeout(options.connect_timeout)
    .build()
    .expect("a plain reqwest client builds");
  let client_id = connection::client_id();
  let mut channels: BTreeSet<String> =
    options.channels.iter().cloned().collect();
  let mut backoff = options.min_backoff;
  let mut connection = 0;
  loop {
    let failed = match open(&options, &http, &client_id).await {
      Ok((mut ws, endpoint)) => match handshake(&mut ws, &options).await {
        Ok((raw, result)) => {
          connection += 1;
          let items = [
            Item::Connected {
              connection,
              endpoint,
            },
            Item::Frame(Frame::new(connection, raw)),
          ];
          if items.into_iter().any(|item| tx.send(item).is_err()) {
            return;
          }
          let started = Instant::now();
          let mut session = Session {
            ws,
            connection,
            options: &options,
            http: &http,
            client_id: &client_id,
            tx: &tx,
            next_id: CONNECT_ID + 1,
            pending: HashMap::new(),
          };
          let end = session
            .pump(
              &mut channels,
              &mut controls,
              refresh_in(result.expires, result.ttl),
            )
            .await;
          let item = match end {
            End::Dropped { code, reason } => Item::Disconnected {
              connection,
              code,
              reason,
            },
            End::Stopped { code, reason } => {
              let _ = tx.send(Item::Stopped { code, reason });
              return;
            }
            End::Gone => return,
          };
          if tx.send(item).is_err() {
            return;
          }
          if started.elapsed() >= STABLE_AFTER {
            backoff = options.min_backoff;
          }
          None
        }
        Err(End::Dropped { reason, .. }) => Some((Stage::ConnectReply, reason)),
        Err(End::Stopped { code, reason }) => {
          let _ = tx.send(Item::Stopped { code, reason });
          return;
        }
        Err(End::Gone) => return,
      },
      Err(failure) => Some(failure),
    };
    if let Some((stage, error)) = failed {
      let item = Item::ConnectFailed {
        stage,
        error,
        retry_in: backoff,
      };
      if tx.send(item).is_err() {
        return;
      }
    }
    sleep(backoff).await;
    backoff = (backoff * 2).min(options.max_backoff);
  }
}

/// When to refresh the token, if it expires: a little before it does.
fn refresh_in(expires: bool, ttl: u32) -> Option<Duration> {
  expires.then(|| Duration::from_secs(ttl.into()) * 9 / 10)
}

/// Asks for a socket and a token, and opens the socket.
async fn open(
  options: &Options,
  http: &::reqwest::Client,
  client_id: &str,
) -> Result<(Ws, String), (Stage, String)> {
  let (endpoint, token) = match &options.setup {
    Setup::Kick { base } => {
      let endpoint = connection::connection(http, base, client_id)
        .await
        .map_err(|e| (Stage::Connection, e.to_string()))?;
      let token = connection::token(http, base, client_id)
        .await
        .map_err(|e| (Stage::Token, e.to_string()))?;
      (endpoint, token)
    }
    Setup::Fixed { endpoint, token } => (endpoint.clone(), token.clone()),
  };
  let ws = match timeout(
    options.connect_timeout,
    ::tokio_tungstenite::connect_async(endpoint.as_str()),
  )
  .await
  {
    Ok(Ok((mut ws, _))) => {
      let connect = Command {
        id: CONNECT_ID,
        body: command::Body::Connect(ConnectRequest {
          token,
          // What Kick's own client calls itself.
          name: "js".into(),
          ..ConnectRequest::default()
        }),
      };
      ws.send(Message::text(connect.encode()))
        .await
        .map_err(|e| (Stage::Transport, format!("send failed: {e}")))?;
      ws
    }
    Ok(Err(e)) => return Err((Stage::Transport, e.to_string())),
    Err(_) => return Err((Stage::Transport, "timed out".into())),
  };
  Ok((ws, endpoint))
}

/// Waits for the connect command's reply, returning its frame and result.
async fn handshake(
  ws: &mut Ws,
  options: &Options,
) -> Result<(String, reply::ConnectResult), End> {
  let wait = async {
    loop {
      match ws.next().await {
        Some(Ok(Message::Text(text))) => {
          for reply in centrifugo::replies(text.as_str()) {
            let reply = reply
              .map_err(|e| End::dropped(format!("reply didn't decode: {e}")))?;
            if let Some(end) = disconnect(&reply) {
              return Err(end);
            }
            if reply.id != CONNECT_ID {
              continue;
            }
            if let Some(error) = reply.error {
              return Err(End::dropped(format!(
                "{} {}",
                error.code, error.message
              )));
            }
            return match reply.body {
              Some(reply::Body::Connect(result)) => {
                Ok((text.as_str().to_owned(), result))
              }
              _ => Err(End::dropped("not a connect result")),
            };
          }
        }
        Some(Ok(Message::Close(close))) => return Err(closed(close)),
        Some(Ok(_)) => {}
        Some(Err(e)) => return Err(End::dropped(e.to_string())),
        None => return Err(End::dropped("stream ended")),
      }
    }
  };
  timeout(options.connect_timeout, wait)
    .await
    .unwrap_or_else(|_| Err(End::dropped("timed out")))
}

/// A push disconnecting the client.
fn disconnect(reply: &Reply) -> Option<End> {
  match &reply.body {
    Some(reply::Body::Push(push::Push {
      body: Some(push::Body::Disconnect(disconnect)),
      ..
    })) => Some(End::coded(disconnect.code, disconnect.reason.clone())),
    _ => None,
  }
}

fn closed(
  close: Option<::tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> End {
  match close {
    Some(close) => {
      End::coded(u16::from(close.code).into(), close.reason.as_str().into())
    }
    None => End::dropped("closed"),
  }
}

/// What a sent command was.
enum Pending {
  Subscribe(String),
  Unsubscribe(String),
  Refresh,
}

/// One connected session.
struct Session<'a> {
  ws: Ws,
  connection: u64,
  options: &'a Options,
  http: &'a ::reqwest::Client,
  client_id: &'a str,
  tx: &'a Tx,
  next_id: u32,
  pending: HashMap<u32, Pending>,
}

impl Session<'_> {
  /// Runs the session until it ends: subscribes, forwards frames, answers
  /// pings, and refreshes the token `refresh_in` from now.
  async fn pump(
    &mut self,
    channels: &mut BTreeSet<String>,
    controls: &mut Controls,
    refresh_in: Option<Duration>,
  ) -> End {
    // What was asked while disconnected.
    while let Ok(control) = controls.try_recv() {
      match control {
        Control::Subscribe(channel) => channels.insert(channel),
        Control::Unsubscribe(channel) => channels.remove(&channel),
      };
    }
    for channel in channels.iter() {
      if let Err(end) = self.subscribe(channel.clone()).await {
        return end;
      }
    }
    let refreshes = matches!(self.options.setup, Setup::Kick { .. });
    let mut refresh_at = refresh_in
      .filter(|_| refreshes)
      .map(|wait| Instant::now() + wait);
    let mut renewing: Option<JoinHandle<Result<String, connection::Error>>> =
      None;
    let mut last = Instant::now();
    loop {
      let refresh_due = async {
        match refresh_at {
          Some(at) => sleep_until(at).await,
          None => pending().await,
        }
      };
      let renewed = async {
        match &mut renewing {
          Some(task) => task.await,
          None => pending().await,
        }
      };
      let result = ::tokio::select! {
        message = self.ws.next() => {
          last = Instant::now();
          self.receive(message, channels).await
        }
        Some(control) = controls.recv() => match control {
          Control::Subscribe(channel) if channels.insert(channel.clone()) => {
            self.subscribe(channel).await.map(|()| None)
          }
          Control::Unsubscribe(channel) if channels.remove(&channel) => {
            self.unsubscribe(channel).await.map(|()| None)
          }
          _ => Ok(None),
        },
        () = refresh_due => {
          refresh_at = None;
          let (http, base, client_id) = (
            self.http.clone(),
            match &self.options.setup {
              Setup::Kick { base } => base.clone(),
              Setup::Fixed { .. } => unreachable!("fixed tokens aren't refreshed"),
            },
            self.client_id.to_owned(),
          );
          renewing = Some(::tokio::spawn(async move {
            connection::token(&http, &base, &client_id).await
          }));
          Ok(None)
        }
        token = renewed => {
          renewing = None;
          match token {
            Ok(Ok(token)) => self.refresh(token).await.map(|()| None),
            Ok(Err(e)) => Err(End::dropped(format!("token refresh failed: {e}"))),
            Err(e) => Err(End::dropped(format!("token refresh failed: {e}"))),
          }
        }
        () = sleep_until(last + self.options.idle_timeout) => {
          Err(End::dropped("idle timeout"))
        }
      };
      match result {
        Ok(Some(wait)) => refresh_at = Some(Instant::now() + wait),
        Ok(None) => {}
        Err(end) => return end,
      }
    }
  }

  /// Handles what the socket gave: `Ok(Some(_))` when a refresh says when
  /// to refresh next.
  async fn receive(
    &mut self,
    message: Option<Result<Message, ::tokio_tungstenite::tungstenite::Error>>,
    channels: &mut BTreeSet<String>,
  ) -> Result<Option<Duration>, End> {
    let connection = self.connection;
    match message {
      Some(Ok(Message::Text(text))) => {
        let replies: Vec<_> = centrifugo::replies(text.as_str()).collect();
        let pings = replies
          .iter()
          .filter(|reply| matches!(reply, Ok(reply) if reply.is_ping()))
          .count();
        if pings > 0 {
          self.send(Message::text("{}")).await?;
        }
        if pings > 0 && pings == replies.len() {
          return Ok(None);
        }
        let frame = Frame::new(connection, text.as_str().to_owned());
        self.yield_item(Item::Frame(frame))?;
        let mut refresh_in = None;
        for reply in replies.into_iter().flatten() {
          if let Some(wait) = self.handle(reply, channels).await? {
            refresh_in = Some(wait);
          }
        }
        Ok(refresh_in)
      }
      Some(Ok(Message::Binary(data))) => {
        let item = Item::Binary {
          connection,
          data: data.to_vec(),
        };
        self.yield_item(item)?;
        Ok(None)
      }
      Some(Ok(Message::Close(close))) => Err(closed(close)),
      Some(Ok(_)) => Ok(None),
      Some(Err(e)) => Err(End::dropped(e.to_string())),
      None => Err(End::dropped("stream ended")),
    }
  }

  /// Acts on one reply.
  async fn handle(
    &mut self,
    reply: Reply,
    channels: &mut BTreeSet<String>,
  ) -> Result<Option<Duration>, End> {
    if let Some(end) = disconnect(&reply) {
      return Err(end);
    }
    let connection = self.connection;
    if let Some(reply::Body::Push(push::Push {
      channel,
      body: Some(push::Body::Unsubscribe(unsubscribe)),
      ..
    })) = reply.body
    {
      let resubscribe =
        unsubscribe.code >= RESUBSCRIBE && channels.contains(&channel);
      if !resubscribe {
        channels.remove(&channel);
      }
      self.yield_item(Item::Unsubscribed {
        connection,
        channel: channel.clone(),
        code: unsubscribe.code,
        reason: unsubscribe.reason,
      })?;
      if resubscribe {
        self.subscribe(channel).await?;
      }
      return Ok(None);
    }
    let Some(pending) = self.pending.remove(&reply.id) else {
      return Ok(None);
    };
    match (pending, reply.error) {
      (Pending::Subscribe(channel), None) => {
        self.yield_item(Item::Subscribed {
          connection,
          channel,
        })?;
      }
      (Pending::Subscribe(channel), Some(error)) => {
        channels.remove(&channel);
        self.yield_item(Item::Unsubscribed {
          connection,
          channel,
          code: error.code,
          reason: error.message,
        })?;
      }
      (Pending::Unsubscribe(channel), _) => {
        self.yield_item(Item::Unsubscribed {
          connection,
          channel,
          code: 0,
          reason: "unsubscribed by the client".into(),
        })?;
      }
      (Pending::Refresh, None) => {
        if let Some(reply::Body::Refresh(result)) = reply.body {
          return Ok(refresh_in(result.expires, result.ttl));
        }
      }
      (Pending::Refresh, Some(error)) => {
        return Err(End::dropped(format!(
          "token refresh refused: {} {}",
          error.code, error.message
        )));
      }
    }
    Ok(None)
  }

  async fn subscribe(&mut self, channel: String) -> Result<(), End> {
    let body = command::Body::Subscribe(SubscribeRequest {
      channel: channel.clone(),
      ..SubscribeRequest::default()
    });
    self.command(body, Pending::Subscribe(channel)).await
  }

  async fn unsubscribe(&mut self, channel: String) -> Result<(), End> {
    let body = command::Body::Unsubscribe(UnsubscribeRequest {
      channel: channel.clone(),
    });
    self.command(body, Pending::Unsubscribe(channel)).await
  }

  async fn refresh(&mut self, token: String) -> Result<(), End> {
    let body = command::Body::Refresh(RefreshRequest { token });
    self.command(body, Pending::Refresh).await
  }

  async fn command(
    &mut self,
    body: command::Body,
    pending: Pending,
  ) -> Result<(), End> {
    let id = self.next_id;
    self.next_id += 1;
    self.pending.insert(id, pending);
    let command = Command { id, body };
    self.send(Message::text(command.encode())).await
  }

  async fn send(&mut self, message: Message) -> Result<(), End> {
    self
      .ws
      .send(message)
      .await
      .map_err(|e| End::dropped(format!("send failed: {e}")))
  }

  fn yield_item(&self, item: Item) -> Result<(), End> {
    self.tx.send(item).map_err(|_| End::Gone)
  }
}
