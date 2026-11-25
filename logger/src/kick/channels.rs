//! The Kick channels to capture: `kick-channels.jsonc` in the install's
//! root, JSONC (see [`::common::config`]), written by `kick-ids` and
//! editable by hand.
//!
//! ```jsonc
//! {
//!   "channels": [
//!     { "slug": "somechannel", "channel_id": 789, "chatroom_id": 123 },
//!   ],
//! }
//! ```
//!
//! [`update`] changes only the channel's own entry, and adds an entry for a
//! new one at the end, so the rest of the file, comments and all, stays as
//! written. Before it was JSONC, the list was [`PLAIN_FILE`], a line per
//! channel, which [`convert_plain`] turns into this.

use {
  ::common::{atomic_file, config, fs::MissingOk},
  ::kick_api::channel,
  ::serde::Deserialize,
  ::serde_json::value::RawValue,
  ::std::{fs, io, path::Path},
};

/// The channel list, in an install's root.
pub const FILE: &str = "kick-channels.jsonc";

/// The channel list before it was JSONC, a line per channel:
///
/// ```text
/// # <slug> <channel id> <chatroom id>
/// somechannel 789 123
/// ```
///
/// Blank lines and `#` comments are skipped.
pub const PLAIN_FILE: &str = "kick-channels";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
  /// Its name in its URL, and its journal's directory: see [`valid_slug`].
  pub slug: String,
  /// The channel's own id, which Kick's chat history is asked by.
  pub channel_id: u64,
  /// The id its chat is read by: `chatrooms.<id>.v2`.
  pub chatroom_id: u64,
}

impl Channel {
  /// As an entry of the list, on one line.
  pub fn json(&self) -> String {
    format!(
      r#"{{ "slug": {}, "channel_id": {}, "chatroom_id": {} }}"#,
      ::serde_json::Value::from(self.slug.as_str()),
      self.channel_id,
      self.chatroom_id
    )
  }

  /// As a line of the [`PLAIN_FILE`].
  pub fn line(&self) -> String {
    format!("{} {} {}", self.slug, self.channel_id, self.chatroom_id)
  }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
  channels: Vec<Channel>,
}

/// A slug: lowercase letters, digits, `-`, `_` and `@` (an app's bot
/// user's starts with it), as Kick's are, so it's also a directory's name
/// on any filesystem, and a URL path's segment.
pub fn valid_slug(slug: &str) -> bool {
  !slug.is_empty()
    && slug.bytes().all(|b| {
      b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_@".contains(&b)
    })
}

/// The channels with journals in Kick's directory `dir` (see
/// [`Kick::dir`](crate::service::Service::dir)), sorted: every directory
/// there named by a [`valid_slug`], whether or not the list still has it.
pub fn journaled(dir: &Path) -> io::Result<Vec<String>> {
  let mut slugs = Vec::new();
  let Some(entries) = fs::read_dir(dir).missing_ok()? else {
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
  let List { channels } = config::parse(text)?;
  for (at, channel) in channels.iter().enumerate() {
    let n = at + 1;
    if !valid_slug(&channel.slug) {
      return Err(format!(
        "channel {n}: a slug is lowercase letters, digits, -, _ and @"
      ));
    }
    if channels[..at]
      .iter()
      .any(|known| known.slug == channel.slug)
    {
      return Err(format!("channel {n}: {} listed twice", channel.slug));
    }
  }
  Ok(channels)
}

/// A list of `channels`, as [`update`] would write it from nothing.
pub fn render(channels: &[Channel]) -> String {
  let mut out = String::from(
    "// The Kick channels to capture: `kick-ids <slug>` looks a channel's\n\
     // ids up and adds it, or add one by hand. A running kick-capture picks\n\
     // a change up within a minute.\n\
     {\n  \"channels\": [\n",
  );
  if channels.is_empty() {
    out.push_str(
      "    // { \"slug\": \"somechannel\", \"channel_id\": 789, \
       \"chatroom_id\": 123 },\n",
    );
  }
  for channel in channels {
    out.push_str(&format!("    {},\n", channel.json()));
  }
  out.push_str("  ],\n}\n");
  out
}

/// A [`PLAIN_FILE`]'s channels, each slug once.
pub fn parse_plain(text: &str) -> Result<Vec<Channel>, String> {
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

/// A [`PLAIN_FILE`]'s text as the list's (see [`render`]). Its comments
/// aren't kept.
pub fn convert_plain(text: &str) -> Result<String, String> {
  Ok(render(&parse_plain(text)?))
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
    return Err("a slug is lowercase letters, digits, -, _ and @");
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
pub async fn look_up(
  slug: &str,
  look_up: impl AsyncFnOnce(&str) -> Result<channel::Channel, String>,
) -> Result<Channel, String> {
  let slug = slug.trim().to_ascii_lowercase();
  if !valid_slug(&slug) {
    return Err("a slug is lowercase letters, digits, -, _ and @".into());
  }
  let found = look_up(&slug).await?;
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

/// Where the list's array of channels is in its text, and each entry, as
/// byte ranges.
#[derive(Deserialize)]
struct Spans<'a> {
  #[serde(borrow)]
  channels: &'a RawValue,
}

/// `text` with `channel` recorded: its entry replaced, or one added at the
/// end of the array, on a line of its own. The rest stays as written. A
/// list that doesn't parse isn't changed.
pub fn record(text: &str, channel: &Channel) -> Result<String, String> {
  let channels = parse(text)?;
  let json = config::as_json(text);
  let at = |part: &str| part.as_ptr() as usize - json.as_ptr() as usize;
  let spans: Spans =
    ::serde_json::from_str(&json).map_err(|e| e.to_string())?;
  let array = spans.channels.get();
  let entries: Vec<&RawValue> =
    ::serde_json::from_str(array).map_err(|e| e.to_string())?;
  let mut out = text.to_owned();
  if let Some(found) = channels.iter().position(|c| c.slug == channel.slug) {
    let entry = entries[found].get();
    let start = at(entry);
    out.replace_range(start..start + entry.len(), &channel.json());
  } else {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let close = at(array) + array.len() - 1;
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |n| n + 1);
    let indent = |at: usize| {
      let before = &text[line_start(at)..at];
      let width = before.len() - before.trim_start().len();
      before[..width].to_owned()
    };
    let entry_indent = match entries.last() {
      Some(last) => indent(at(last.get())),
      None => indent(close) + "  ",
    };
    let new = format!("{entry_indent}{},", channel.json());
    // Inserted from the end, so what's before stays where it was.
    match text[line_start(close)..close].trim().is_empty() {
      // `]` on a line of its own: the entry goes on a line before it.
      true => out.insert_str(line_start(close), &format!("{new}{eol}")),
      false => {
        let after = format!("{eol}{new}{eol}{}", indent(close));
        out.insert_str(close, &after);
      }
    }
    // The last entry, followed by a comma if it isn't yet.
    if let Some(last) = entries.last() {
      let end = at(last.get()) + last.get().len();
      if !text[end..close].trim_start().starts_with(',') {
        out.insert(end, ',');
      }
    }
  }
  // Whatever was there, what's written must read back.
  let broken = |e| format!("recording {} would break it: {e}", channel.slug);
  parse(&out).map_err(broken)?;
  Ok(out)
}

/// Re-reads `path` and records `channel` in it, replacing the file whole
/// (see [`atomic_file`]); with no file, starting from [`render`]'s.
pub fn update(path: &Path, channel: &Channel) -> Result<(), String> {
  let in_path = |e: String| format!("{}: {e}", path.display());
  let text = fs::read_to_string(path)
    .missing_ok()
    .map_err(|e| in_path(e.to_string()))?
    .unwrap_or_else(|| render(&[]));
  let text = record(&text, channel).map_err(in_path)?;
  atomic_file::replace(path, text).map_err(|e| in_path(e.to_string()))
}
