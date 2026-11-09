//! dgg chat: its socket, at [`DEFAULT_URL`], and the [events](event) it
//! sends.

#[cfg(feature = "chat")]
pub mod client;
pub mod event;

#[cfg(feature = "chat")]
pub use client::{Client, Frame, Item, Options};
pub use event::Event;

/// The chat socket's URL.
pub const DEFAULT_URL: &str = "wss://chat.destiny.gg/ws";
/// The Origin dgg's own site connects with.
pub const DEFAULT_ORIGIN: &str = "https://www.destiny.gg";
