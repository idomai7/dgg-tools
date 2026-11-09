//! destiny.gg's APIs, shared by the dgg tools: lightweight, so any tool can
//! depend on it.
//!
//! - [`chat`]: the chat socket. Its client (feature `client`) reads raw
//!   frames; its protocol (feature `protocol`) decodes them.
//! - `assets` (feature `assets`): chat's emotes and flairs.
//!
//! Each feature brings in one part, so a tool pulls in only what it uses.

#[cfg(feature = "assets")]
pub mod assets;
pub mod chat;
