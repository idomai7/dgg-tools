//! The Kick variant: Kick's realtime (Centrifugo) frames, versioned on its
//! own as `kick.<major>.<minor>`, so its changes never break dgg's files.
//!
//! The header is `journal kick.<major>.<minor> ; endpoint=<url>
//! channel=<slug>` (see [`Header`](super::Header)): `endpoint` is the
//! socket the file's connections are to, the same URL its `open` lines
//! record, and `channel` the slug they're subscribed for. Each channel has a
//! journal root of its own (see [`channel_root`](crate::layout::channel_root)),
//! and a file only ever holds connections to one endpoint: a different one
//! starts the next file (see [`writer::kick`](crate::writer::kick)).

pub mod v0;

/// The newest version of each major this crate has a decoder for.
pub const READABLE: &[super::Version] = &[v0::VERSION];
