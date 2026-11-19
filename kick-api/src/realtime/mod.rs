//! Kick's realtime chat, served by Centrifugo. Its frames decode into
//! [Centrifugo's client protocol](centrifugo) (feature `centrifugo`). The
//! client (feature `client`) reads them raw, after Kick's
//! [setup](connection).

pub mod centrifugo;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod connection;
pub mod rest;

#[cfg(feature = "client")]
pub use client::{Client, Frame, Item, Options, Setup, Stage};
pub use rest::Rest;
