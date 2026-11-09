//! dgg chat: its socket, at [`DEFAULT_URL`]. The `client` reads raw frames
//! off it; `protocol` decodes them. Each has a feature of its own, and
//! neither needs the other.

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "protocol")]
pub mod protocol;

#[cfg(feature = "client")]
pub use client::{Client, Frame, Item, NotConnected, Options};

/// The chat socket's URL.
pub const DEFAULT_URL: &str = "wss://chat.destiny.gg/ws";
/// The Origin dgg's own site connects with.
pub const DEFAULT_ORIGIN: &str = "https://www.destiny.gg";
