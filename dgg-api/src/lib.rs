//! destiny.gg's APIs, shared by the dgg tools: lightweight, so any tool can
//! depend on it.
//!
//! - [`chat`]: the chat socket; its client needs the `chat` feature.
//!
//! Each feature brings in the I/O for one kind of API, so a tool pulls in
//! only what it talks to.

pub mod chat;
