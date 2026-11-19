//! What a server sends: a [`Reply`], answering a command or carrying a push.

use {
  super::push::{Publication, Push},
  crate::realtime::rest::{self, Rest},
  ::serde::{Deserialize, Deserializer, de::Error as _},
  ::serde_json::Value,
  ::std::collections::BTreeMap,
};

/// A reply: to a command (`id` set), or a push (`id` 0). One with nothing
/// set is the server's ping, which the client answers with an empty
/// command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
  /// The command answered; 0 for a push or a ping.
  pub id: u32,
  pub error: Option<Error>,
  /// The result, or the push; `None` for an error or a ping, or one this
  /// crate doesn't type, which is then in `rest`.
  pub body: Option<Body>,
  pub rest: Rest,
}

/// What a reply carries: at most one of these is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
  Push(Push),
  Connect(ConnectResult),
  Subscribe(SubscribeResult),
  Unsubscribe(UnsubscribeResult),
  History(HistoryResult),
  Refresh(RefreshResult),
  SubRefresh(SubRefreshResult),
}

impl Reply {
  /// The server's ping: a reply with nothing set.
  pub fn is_ping(&self) -> bool {
    self.id == 0
      && self.error.is_none()
      && self.body.is_none()
      && self.rest.is_none()
  }
}

impl<'de> Deserialize<'de> for Reply {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    #[derive(Deserialize)]
    struct Wire {
      #[serde(default)]
      id: u32,
      error: Option<Error>,
      push: Option<Push>,
      connect: Option<ConnectResult>,
      subscribe: Option<SubscribeResult>,
      unsubscribe: Option<UnsubscribeResult>,
      history: Option<HistoryResult>,
      refresh: Option<RefreshResult>,
      sub_refresh: Option<SubRefreshResult>,
      #[serde(flatten, deserialize_with = "rest::deserialize")]
      rest: Rest,
    }
    let wire = Wire::deserialize(deserializer)?;
    let mut bodies = [
      wire.push.map(Body::Push),
      wire.connect.map(Body::Connect),
      wire.subscribe.map(Body::Subscribe),
      wire.unsubscribe.map(Body::Unsubscribe),
      wire.history.map(Body::History),
      wire.refresh.map(Body::Refresh),
      wire.sub_refresh.map(Body::SubRefresh),
    ]
    .into_iter()
    .flatten();
    let body = bodies.next();
    if bodies.next().is_some() {
      return Err(D::Error::custom("a reply with more than one result"));
    }
    Ok(Self {
      id: wire.id,
      error: wire.error,
      body,
      rest: wire.rest,
    })
  }
}

/// A command's error. Codes below 400 are Centrifugo's own; 400 to 1999
/// are the application's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Error {
  pub code: u32,
  pub message: String,
  /// Whether trying again may succeed.
  pub temporary: bool,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// `connect`'s result: the connection's session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ConnectResult {
  /// The connection's client id.
  pub client: String,
  /// The server's version.
  pub version: String,
  /// Whether the connection expires, and must be refreshed before `ttl`
  /// seconds pass.
  pub expires: bool,
  pub ttl: u32,
  pub data: Option<Value>,
  /// Subscriptions the server made for the connection, by channel.
  pub subs: BTreeMap<String, SubscribeResult>,
  /// Seconds between the server's pings; 0 for none.
  pub ping: u32,
  /// Whether the server expects each ping answered.
  pub pong: bool,
  pub session: String,
  pub node: String,
  /// The server's time, in Unix ms.
  pub time: i64,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// `subscribe`'s result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SubscribeResult {
  /// Whether the subscription expires, and must be refreshed before `ttl`
  /// seconds pass.
  pub expires: bool,
  pub ttl: u32,
  /// Whether publications missed while away can be recovered.
  pub recoverable: bool,
  pub epoch: String,
  /// Recovered publications.
  pub publications: Vec<Publication>,
  pub recovered: bool,
  pub offset: u64,
  pub positioned: bool,
  pub data: Option<Value>,
  pub was_recovering: bool,
  pub delta: bool,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// `unsubscribe`'s result, which has nothing in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UnsubscribeResult {
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// `history`'s result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HistoryResult {
  pub publications: Vec<Publication>,
  pub epoch: String,
  pub offset: u64,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// `refresh`'s result: the connection's new expiry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RefreshResult {
  pub client: String,
  pub version: String,
  pub expires: bool,
  pub ttl: u32,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// `sub_refresh`'s result: a subscription's new expiry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SubRefreshResult {
  pub expires: bool,
  pub ttl: u32,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}
