//! Centrifugo's client protocol, in JSON: the [replies](reply) and
//! [pushes](push) a server sends, decoded, and the [commands](command) a
//! client sends, encoded. Types and names follow Centrifugo's
//! `client.proto` (github.com/centrifugal/protocol): messages are structs,
//! and a message's fields of which at most one is set are an enum.
//!
//! As in protobuf's JSON, a field left at its default (0, `false`, `""`)
//! may be left out, so such fields default rather than being required.
//! Fields a type doesn't name are kept in its `rest`.
//!
//! Kept apart from the client, which deals in raw frames: a tool reading a
//! journal decodes frames without a socket.

pub mod command;
pub mod push;
pub mod reply;

pub use {command::Command, push::Push, reply::Reply};

/// The replies in a frame. Centrifugo may batch several into one frame,
/// one per line.
pub fn replies(
  frame: &str,
) -> impl Iterator<Item = Result<Reply, ::serde_json::Error>> + '_ {
  frame
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(::serde_json::from_str)
}
