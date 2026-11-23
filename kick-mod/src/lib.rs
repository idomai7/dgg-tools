//! A chat client and moderation tools for Kick channels: what the binary
//! is made of.
//!
//! - [`account`]: the moderator's Kick account, and signing in.
//! - [`config`]: `kick-mod.toml`, the settings, edited by hand.
//! - [`channels`]: `data/kick-mod/channels.json`, the channels kick-mod
//!   knows, which it writes too.
//! - [`chat`]: live chat, a connection per channel.
//! - [`serve`]: the page.

pub mod account;
pub mod channels;
pub mod chat;
pub mod config;
pub mod serve;
