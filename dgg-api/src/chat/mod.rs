//! dgg chat: its socket, at [`DEFAULT_URL`].

pub mod client;

pub use client::{Client, DEFAULT_ORIGIN, DEFAULT_URL, Frame, Item, Options};
