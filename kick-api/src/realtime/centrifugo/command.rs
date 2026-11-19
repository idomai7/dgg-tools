//! What a client sends: a [`Command`]. Only the requests Kick's client
//! needs are here.

use {::serde::Serialize, ::serde_json::Value};

/// A command. The server's reply carries the same `id`, which must not be
/// 0. The server's ping is answered not with a command but with `{}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Command {
  pub id: u32,
  #[serde(flatten)]
  pub body: Body,
}

/// What a command asks: exactly one of these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Body {
  Connect(ConnectRequest),
  Subscribe(SubscribeRequest),
  Unsubscribe(UnsubscribeRequest),
  Refresh(RefreshRequest),
}

impl Command {
  /// The command as JSON, ready to send.
  pub fn encode(&self) -> String {
    ::serde_json::to_string(self).expect("commands always encode")
  }
}

/// Opens the connection's session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConnectRequest {
  #[serde(skip_serializing_if = "String::is_empty")]
  pub token: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub data: Option<Value>,
  /// The client's name; Kick's own sends `js`.
  #[serde(skip_serializing_if = "String::is_empty")]
  pub name: String,
  #[serde(skip_serializing_if = "String::is_empty")]
  pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SubscribeRequest {
  pub channel: String,
  #[serde(skip_serializing_if = "String::is_empty")]
  pub token: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UnsubscribeRequest {
  pub channel: String,
}

/// Renews the connection with a new token before it expires.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RefreshRequest {
  pub token: String,
}
