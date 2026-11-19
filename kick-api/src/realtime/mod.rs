//! Kick's realtime chat, served by Centrifugo. Its frames decode into
//! [Centrifugo's client protocol](centrifugo) (feature `centrifugo`). The
//! client (feature `client`) reads them raw, after Kick's
//! [setup](connection). What Kick publishes decodes into its
//! [events](event) (feature `protocol`).

pub mod centrifugo;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod connection;
#[cfg(feature = "protocol")]
pub mod event;
pub mod rest;

#[cfg(feature = "client")]
pub use client::{Client, Frame, Item, Options, Setup, Stage};
#[cfg(feature = "protocol")]
pub use event::Event;
pub use rest::Rest;
