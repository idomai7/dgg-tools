//! What chat's frames mean: the [events](event) it sends, decoded.
//!
//! Kept apart from the client (`chat::client`), which deals only in raw
//! frames: a tool that keeps frames as they are (the logger) needs no
//! decoding, and one that decodes them (from a journal, say) needs no
//! socket.

pub mod event;

pub use event::Event;
