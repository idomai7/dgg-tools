//! Kick's APIs, shared by the Kick tools: lightweight, so any tool can
//! depend on it.
//!
//! - `realtime`: Kick's realtime chat. Feature `centrifugo` decodes its
//!   frames into Centrifugo's client protocol, feature `protocol` what Kick
//!   publishes into its events, and feature `client` reads the frames.
//! - `channel` (feature `channel`): the unofficial channel lookup, the only
//!   place a channel's chatroom id is found. Feature `lookup` asks it.
//! - `official` (feature `official`): Kick's official public API and
//!   signing in to it.
//! - `web` (feature `web`): `web.kick.com`'s unofficial API, a channel's
//!   chat history.
//! - `rest`, shared by the decoders: the fields a type doesn't name.
//!
//! Timestamps are unix milliseconds, from [`common::time`].
//!
//! Each feature brings in one part, so a tool pulls in only what it uses.

#[cfg(feature = "channel")]
pub mod channel;
#[cfg(feature = "official")]
pub mod official;
#[cfg(feature = "centrifugo")]
pub mod realtime;
#[cfg(any(feature = "centrifugo", feature = "official"))]
pub mod rest;
#[cfg(any(feature = "protocol", feature = "official"))]
mod time;
#[cfg(feature = "web")]
pub mod web;

#[cfg(any(feature = "centrifugo", feature = "official"))]
pub use rest::Rest;
