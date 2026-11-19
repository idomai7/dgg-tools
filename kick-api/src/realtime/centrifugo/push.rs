//! What a server sends unasked: a [`Push`], to a channel or to the
//! connection.

use {
  crate::realtime::rest::{self, Rest},
  ::serde::{Deserialize, Deserializer, de::Error as _},
  ::serde_json::Value,
  ::std::collections::BTreeMap,
};

/// A push: to a channel, or to the connection (`channel` empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
  pub channel: String,
  /// What's pushed; `None` for one this crate doesn't type, which is then
  /// in `rest`.
  pub body: Option<Body>,
  pub rest: Rest,
}

/// What a push carries: at most one of these is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
  /// `pub`: something published to the channel.
  Publication(Publication),
  Join(Join),
  Leave(Leave),
  /// The server unsubscribed the connection from the channel.
  Unsubscribe(Unsubscribe),
  /// Data sent to the connection, not to a channel.
  Message(Message),
  /// The server subscribed the connection to the channel.
  Subscribe(Subscribe),
  /// The server is closing the connection.
  Disconnect(Disconnect),
  /// The server refreshed the connection.
  Refresh(Refresh),
}

impl<'de> Deserialize<'de> for Push {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    #[derive(Deserialize)]
    struct Wire {
      #[serde(default)]
      channel: String,
      #[serde(rename = "pub")]
      publication: Option<Publication>,
      join: Option<Join>,
      leave: Option<Leave>,
      unsubscribe: Option<Unsubscribe>,
      message: Option<Message>,
      subscribe: Option<Subscribe>,
      disconnect: Option<Disconnect>,
      refresh: Option<Refresh>,
      #[serde(flatten, deserialize_with = "rest::deserialize")]
      rest: Rest,
    }
    let wire = Wire::deserialize(deserializer)?;
    let mut bodies = [
      wire.publication.map(Body::Publication),
      wire.join.map(Body::Join),
      wire.leave.map(Body::Leave),
      wire.unsubscribe.map(Body::Unsubscribe),
      wire.message.map(Body::Message),
      wire.subscribe.map(Body::Subscribe),
      wire.disconnect.map(Body::Disconnect),
      wire.refresh.map(Body::Refresh),
    ]
    .into_iter()
    .flatten();
    let body = bodies.next();
    if bodies.next().is_some() {
      return Err(D::Error::custom("a push with more than one body"));
    }
    Ok(Self {
      channel: wire.channel,
      body,
      rest: wire.rest,
    })
  }
}

/// Something published to a channel. For Kick, `data` is the event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Publication {
  /// What was published, as JSON.
  pub data: Value,
  /// Who published it, if a client did.
  pub info: Option<ClientInfo>,
  /// Its place in the channel's history, if the channel keeps one.
  pub offset: u64,
  pub tags: BTreeMap<String, String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A client connected to a channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ClientInfo {
  pub user: String,
  pub client: String,
  pub conn_info: Option<Value>,
  pub chan_info: Option<Value>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A client joined the channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Join {
  pub info: ClientInfo,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A client left the channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Leave {
  pub info: ClientInfo,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// The server unsubscribed the connection. Codes 2500 and up mean
/// subscribe again; below that, don't.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Unsubscribe {
  pub code: u32,
  pub reason: String,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// Data sent to the connection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Message {
  pub data: Value,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// The server subscribed the connection to a channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Subscribe {
  pub recoverable: bool,
  pub epoch: String,
  pub offset: u64,
  pub positioned: bool,
  pub data: Option<Value>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// The server is closing the connection. Codes 3000 to 3499 mean
/// reconnect; 3500 and up, don't.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Disconnect {
  pub code: u32,
  pub reason: String,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// The server refreshed the connection: its new expiry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Refresh {
  pub expires: bool,
  pub ttl: u32,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}
