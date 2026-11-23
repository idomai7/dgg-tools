//! A chat client and moderation tools for Kick channels: what the binary
//! is made of.
//!
//! - [`config`]: `kick-mod.toml`, the settings, edited by hand.
//! - [`channels`]: `data/kick-mod/channels.json`, the channels kick-mod
//!   knows, which it writes too.
//! - [`serve`]: the page.

pub mod channels;
pub mod config;
pub mod serve;
