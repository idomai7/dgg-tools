//! The Kick channels to capture: `kick-channels` in the install's root, a
//! line per channel, written by `kick-ids` and editable by hand.
//!
//! ```text
//! # <slug> <channel id> <chatroom id>
//! somechannel 789 123
//! ```
//!
//! Blank lines and `#` comments are skipped. [`update`] changes only the
//! channel's own line, keeping its comment, and adds a line for a new one,
//! so the rest of the file stays as written.

use {
  ::common::{atomic_file, fs::MissingOk},
  ::kick_api::channel,
  ::logger_journal::layout::kick_root,
  ::std::{fs, io, path::Path},
};

/// The channel list, in an install's root.
pub const FILE: &str = "kick-channels";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
  /// Its name in its URL, and its journal's directory: see [`valid_slug`].
  pub slug: String,
  /// The channel's own id, which Kick's chat history is asked by.
  pub channel_id: u64,
  /// The id its chat is read by: `chatrooms.<id>.v2`.
  pub chatroom_id: u64,
}

impl Channel {
  /// As a line of the list.
  pub fn line(&self) -> String {
    format!("{} {} {}", self.slug, self.channel_id, self.chatroom_id)
  }
}

/// A slug: lowercase letters, digits, `-` and `_`, as Kick's are, so it's
/// also a directory's name on any filesystem.
pub fn valid_slug(slug: &str) -> bool {
  !slug.is_empty()
    && slug.bytes().all(|b| {
      b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
    })
}

/// The channels with journals under dgg's journal root `journal_root`
/// (see [`kick_root`]), sorted: every directory there named by a
/// [`valid_slug`], whether or not the list still has it.
pub fn journaled(journal_root: &Path) -> io::Result<Vec<String>> {
  let mut slugs = Vec::new();
  let Some(entries) = fs::read_dir(kick_root(journal_root)).missing_ok()?
  else {
    return Ok(slugs);
  };
  for entry in entries {
    let entry = entry?;
    let slug = entry.file_name().to_string_lossy().into_owned();
    if entry.file_type()?.is_dir() && valid_slug(&slug) {
      slugs.push(slug);
    }
  }
  slugs.sort();
  Ok(slugs)
}

/// A list's channels, each slug once.
pub fn parse(text: &str) -> Result<Vec<Channel>, String> {
  let mut channels: Vec<Channel> = Vec::new();
  for (number, line) in text.lines().enumerate() {
    let Some(channel) = parse_line(line)
      .map_err(|why| format!("line {}: {why}: {line:?}", number + 1))?
    else {
      continue;
    };
    if channels.iter().any(|known| known.slug == channel.slug) {
      return Err(format!("line {}: listed twice: {line:?}", number + 1));
    }
    channels.push(channel);
  }
  Ok(channels)
}

/// A line's channel: none for a blank line or a comment.
fn parse_line(line: &str) -> Result<Option<Channel>, &'static str> {
  let line = line.split('#').next().unwrap_or_default().trim();
  if line.is_empty() {
    return Ok(None);
  }
  let fields: Vec<&str> = line.split_whitespace().collect();
  let [slug, channel_id, chatroom_id] = fields[..] else {
    return Err("not `<slug> <channel id> <chatroom id>`");
  };
  if !valid_slug(slug) {
    return Err("a slug is lowercase letters, digits, - and _");
  }
  let (Ok(channel_id), Ok(chatroom_id)) =
    (channel_id.parse(), chatroom_id.parse())
  else {
    return Err("an id is a number");
  };
  Ok(Some(Channel {
    slug: slug.to_owned(),
    channel_id,
    chatroom_id,
  }))
}

/// `slug`'s ids, asked of `look_up`: in practice [`channel::look_up`],
/// Kick's unofficial lookup. The slug is taken in any case.
pub fn look_up(
  slug: &str,
  look_up: impl FnOnce(&str) -> Result<channel::Channel, String>,
) -> Result<Channel, String> {
  let slug = slug.trim().to_ascii_lowercase();
  if !valid_slug(&slug) {
    return Err("a slug is letters, digits, - and _".into());
  }
  let found = look_up(&slug)?;
  if found.slug != slug {
    return Err(format!("the lookup answered for {:?}", found.slug));
  }
  Ok(Channel {
    slug,
    channel_id: found.id,
    chatroom_id: found.chatroom.id,
  })
}

/// The channels in `path`: none if there's no file.
pub fn load(path: &Path) -> Result<Vec<Channel>, String> {
  let in_path = |e: String| format!("{}: {e}", path.display());
  let text = fs::read_to_string(path)
    .missing_ok()
    .map_err(|e| in_path(e.to_string()))?;
  text.map_or(Ok(Vec::new()), |text| parse(&text).map_err(in_path))
}

/// `text` with `channel` recorded: its line replaced, keeping any comment
/// on it, or added at the end. A list that doesn't parse isn't changed.
pub fn record(text: &str, channel: &Channel) -> Result<String, String> {
  parse(text)?;
  let mut out = String::with_capacity(text.len() + 64);
  let mut found = false;
  for line in text.split_inclusive('\n') {
    let ours = matches!(
      parse_line(line),
      Ok(Some(known)) if known.slug == channel.slug
    );
    if !ours {
      out.push_str(line);
      continue;
    }
    found = true;
    out.push_str(&channel.line());
    let body = line.trim_end_matches(['\r', '\n']);
    if let Some(comment) = body.find('#').map(|at| &body[at..]) {
      out.push(' ');
      out.push_str(comment);
    }
    out.push_str(&line[body.len()..]);
  }
  if !found {
    if !out.is_empty() && !out.ends_with('\n') {
      out.push('\n');
    }
    out.push_str(&channel.line());
    out.push('\n');
  }
  Ok(out)
}

/// Re-reads `path` and records `channel` in it, replacing the file whole
/// (see [`atomic_file`]).
pub fn update(path: &Path, channel: &Channel) -> Result<(), String> {
  let in_path = |e: String| format!("{}: {e}", path.display());
  let text = fs::read_to_string(path)
    .missing_ok()
    .map_err(|e| in_path(e.to_string()))?
    .unwrap_or_default();
  let text = record(&text, channel).map_err(in_path)?;
  atomic_file::replace(path, text).map_err(|e| in_path(e.to_string()))
}
