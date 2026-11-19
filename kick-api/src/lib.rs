//! Kick's APIs, shared by the Kick tools: lightweight, so any tool can
//! depend on it.
//!
//! - `channel` (feature `channel`): the unofficial channel lookup, the only
//!   place a channel's chatroom id is found.
//!
//! Each feature brings in one part, so a tool pulls in only what it uses.

#[cfg(feature = "channel")]
pub mod channel;
