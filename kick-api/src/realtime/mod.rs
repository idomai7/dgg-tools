//! Kick's realtime chat, served by Centrifugo. Its frames decode into
//! [Centrifugo's client protocol](centrifugo) (feature `centrifugo`).

pub mod centrifugo;
pub mod rest;

pub use rest::Rest;
