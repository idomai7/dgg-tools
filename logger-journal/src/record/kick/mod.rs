//! The Kick variant: Kick's realtime (Centrifugo) frames, versioned on its
//! own as `kick.<major>.<minor>`, so its changes never break dgg's files.
//!
//! The header is `journal kick.<major>.<minor> ; endpoint=<url>
//! channel=<slug>`, ending at RS LF (see [`Header`](super::Header)): `endpoint` is the
//! socket the file's connections are to, the same URL its `open` records
//! hold, and `channel` the slug they're subscribed for. Each channel has a
//! journal root of its own (see [`channel_root`](crate::layout::channel_root)),
//! and a file only ever holds connections to one endpoint: a different one
//! starts the next file (see [`writer::kick`](crate::writer::kick)).

pub mod v0;

/// The versions this crate has a decoder for: within major 0, each minor
/// (`kick.0.0`'s files are read by `kick.0.1`'s decoder, as it only added a
/// kind); from major 1 on, the newest of each major.
pub const READABLE: &[super::Version] =
  &[super::Version { major: 0, minor: 0 }, v0::VERSION];
