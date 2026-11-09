//! destiny.gg's APIs, shared by the dgg tools: lightweight, so any tool can
//! depend on it.
//!
//! - [`chat`]: the chat socket. Its client (feature `client`) reads raw
//!   frames; its protocol (feature `protocol`) decodes them.
//!
//! Each feature brings in one part, so a tool pulls in only what it uses.

pub mod chat;
