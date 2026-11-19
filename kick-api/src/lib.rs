//! Kick's APIs, shared by the Kick tools: lightweight, so any tool can
//! depend on it.
//!
//! - `realtime`: Kick's realtime chat. Feature `centrifugo` decodes its
//!   frames into Centrifugo's client protocol, feature `protocol` what Kick
//!   publishes into its events, and feature `client` reads the frames.
//! - `channel` (feature `channel`): the unofficial channel lookup, the only
//!   place a channel's chatroom id is found.
//!
//! Each feature brings in one part, so a tool pulls in only what it uses.

#[cfg(feature = "channel")]
pub mod channel;
#[cfg(feature = "centrifugo")]
pub mod realtime;
