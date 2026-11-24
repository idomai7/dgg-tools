//! A chat client and moderation tools for Kick channels: what the binary
//! is made of.
//!
//! - [`account`]: the moderator's Kick account, and signing in.
//! - [`actions`]: what a moderator does from the page.
//! - [`config`]: `kick-mod.env.jsonc`, the settings and secrets, edited by
//!   hand.
//! - [`channels`]: `data/kick-mod/channels.json`, the channels kick-mod
//!   knows, which it writes too.
//! - [`chat`]: live chat, a connection per channel, journaled in
//!   `data/kick-mod/journal/`.
//! - [`history`]: a user's history in a channel, from the journal and the
//!   mod log.
//! - [`logger`]: the logger's `serve`, which kick-mod reads Kick from.
//! - [`modlog`]: `data/kick-mod/modlog.jsonl`, every action taken.
//! - [`serve`]: the page.

pub mod account;
pub mod actions;
pub mod channels;
pub mod chat;
pub mod config;
pub mod history;
pub mod logger;
pub mod modlog;
pub mod serve;
