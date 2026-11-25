//! The dgg channels to capture: `dgg-channels.jsonc` in the install's
//! root, JSONC (see [`::common::config`]), edited by hand. Each is a chat
//! speaking dgg's protocol at a WebSocket URL of its own.
//!
//! ```jsonc
//! {
//!   "channels": [
//!     {
//!       "name": "destiny.gg",
//!       "url": "wss://chat.destiny.gg/ws",
//!       "origin": "https://www.destiny.gg", // or null for none
//!     },
//!   ],
//! }
//! ```
//!
//! With no file, the list is [`Channel::destiny`] alone, so a host that
//! never writes one captures destiny.gg; an install writes one (see
//! [`render`]). Before it was JSONC, the list was [`PLAIN_FILE`], a line
//! per channel, which [`convert_plain`] turns into this.

use {
  ::common::{config, fs::MissingOk},
  ::dgg_api::chat::{DEFAULT_ORIGIN, DEFAULT_URL},
  ::serde::Deserialize,
  ::serde_json::Value,
  ::std::{fs, path::Path},
};

/// The channel list, in an install's root.
pub const FILE: &str = "dgg-channels.jsonc";

/// The channel list before it was JSONC, a line per channel:
///
/// ```text
/// # <name> <WebSocket URL> [<Origin header>, or - for none]
/// destiny.gg wss://chat.destiny.gg/ws https://www.destiny.gg
/// ```
///
/// Blank lines and `#` comments are skipped.
pub const PLAIN_FILE: &str = "dgg-channels";

/// destiny.gg's channel's name.
pub const DESTINY: &str = "destiny.gg";

/// Directories of dgg's journal before it was by channel, which no channel
/// may be named, so one is never mistaken for the other.
pub const RESERVED: [&str; 4] = ["live", "archive", "backfill", "control"];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
  /// Its journal's directory, the site's domain by convention: see
  /// [`valid_name`].
  pub name: String,
  /// Its chat's socket.
  pub url: String,
  /// The `Origin` header its socket is opened with, if any.
  pub origin: Option<String>,
}

impl Channel {
  /// destiny.gg's chat, the list's one channel when there's no file.
  pub fn destiny() -> Self {
    Self {
      name: DESTINY.into(),
      url: DEFAULT_URL.into(),
      origin: Some(DEFAULT_ORIGIN.into()),
    }
  }

  /// As an entry of the list, indented by `indent`, its fields a line
  /// each.
  pub fn json(&self, indent: &str) -> String {
    let origin = match &self.origin {
      Some(origin) => Value::from(origin.as_str()),
      None => Value::Null,
    };
    format!(
      "{indent}{{\n{indent}  \"name\": {},\n{indent}  \"url\": {},\n\
       {indent}  \"origin\": {origin},\n{indent}}}",
      Value::from(self.name.as_str()),
      Value::from(self.url.as_str()),
    )
  }

  /// As a line of the [`PLAIN_FILE`].
  pub fn line(&self) -> String {
    let origin = self.origin.as_deref().unwrap_or("-");
    format!("{} {} {origin}", self.name, self.url)
  }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
  channels: Vec<Channel>,
}

/// A name: lowercase letters, digits, `-`, `_` and `.`, not starting or
/// ending with `.` and with no `..`, so it's a directory's name on any
/// filesystem (Windows drops a trailing dot) and a URL path's segment, and
/// never `.` or `..`. Nor is it one of [`RESERVED`].
pub fn valid_name(name: &str) -> bool {
  !name.is_empty()
    && name.bytes().all(|b| {
      b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b)
    })
    && !name.starts_with('.')
    && !name.ends_with('.')
    && !name.contains("..")
    && !RESERVED.contains(&name)
}

/// A list's channels, each name once, at least one.
pub fn parse(text: &str) -> Result<Vec<Channel>, String> {
  let List { channels } = config::parse(text)?;
  for (at, channel) in channels.iter().enumerate() {
    let why = |why: &str| format!("channel {}: {why}", at + 1);
    check(channel).map_err(why)?;
    if channels[..at]
      .iter()
      .any(|known| known.name == channel.name)
    {
      return Err(why(&format!("{} listed twice", channel.name)));
    }
  }
  if channels.is_empty() {
    return Err("no channels listed".into());
  }
  Ok(channels)
}

/// What's wrong with `channel`, if anything.
fn check(channel: &Channel) -> Result<(), &'static str> {
  if !valid_name(&channel.name) {
    return Err(
      "a name is lowercase letters, digits, -, _ and ., and not live, \
       archive, backfill or control",
    );
  }
  let url = &channel.url;
  if !url.starts_with("wss://") && !url.starts_with("ws://") {
    return Err("a URL starts with wss:// or ws://");
  }
  if let Some(origin) = &channel.origin
    && !origin.starts_with("https://")
    && !origin.starts_with("http://")
  {
    return Err("an Origin starts with https:// or http://, or is null");
  }
  Ok(())
}

/// A list of `channels`, as an install writes it.
pub fn render(channels: &[Channel]) -> String {
  let mut out = String::from(
    "// The dgg channels to capture, each a chat speaking dgg's protocol:\n\
     // its name (its journal's directory), its WebSocket URL, and the\n\
     // Origin header it's opened with (null for none). A running\n\
     // supervisor picks a change up within a minute.\n\
     {\n  \"channels\": [\n",
  );
  for channel in channels {
    out.push_str(&channel.json("    "));
    out.push_str(",\n");
  }
  out.push_str("  ],\n}\n");
  out
}

/// A [`PLAIN_FILE`]'s channels, each name once, at least one.
pub fn parse_plain(text: &str) -> Result<Vec<Channel>, String> {
  let mut channels: Vec<Channel> = Vec::new();
  for (number, line) in text.lines().enumerate() {
    let Some(channel) = parse_line(line)
      .map_err(|why| format!("line {}: {why}: {line:?}", number + 1))?
    else {
      continue;
    };
    if channels.iter().any(|known| known.name == channel.name) {
      return Err(format!("line {}: listed twice: {line:?}", number + 1));
    }
    channels.push(channel);
  }
  if channels.is_empty() {
    return Err("no channels listed".into());
  }
  Ok(channels)
}

/// A [`PLAIN_FILE`]'s text as the list's (see [`render`]). Its comments
/// aren't kept.
pub fn convert_plain(text: &str) -> Result<String, String> {
  Ok(render(&parse_plain(text)?))
}

/// A plain line's channel: none for a blank line or a comment.
fn parse_line(line: &str) -> Result<Option<Channel>, &'static str> {
  let line = line.split('#').next().unwrap_or_default().trim();
  if line.is_empty() {
    return Ok(None);
  }
  let fields: Vec<&str> = line.split_whitespace().collect();
  let (name, url, origin) = match fields[..] {
    [name, url] | [name, url, "-"] => (name, url, None),
    [name, url, origin] => (name, url, Some(origin)),
    _ => return Err("not `<name> <URL> [<Origin>|-]`"),
  };
  let channel = Channel {
    name: name.to_owned(),
    url: url.to_owned(),
    origin: origin.map(str::to_owned),
  };
  match check(&channel) {
    Err(why) if why.starts_with("an Origin") => {
      Err("an Origin starts with https:// or http://, or is -")
    }
    Err(why) => Err(why),
    Ok(()) => Ok(Some(channel)),
  }
}

/// The channels in `path`: [`Channel::destiny`] alone if there's no file.
pub fn load(path: &Path) -> Result<Vec<Channel>, String> {
  let in_path = |e: String| format!("{}: {e}", path.display());
  let text = fs::read_to_string(path)
    .missing_ok()
    .map_err(|e| in_path(e.to_string()))?;
  text.map_or(Ok(vec![Channel::destiny()]), |text| {
    parse(&text).map_err(in_path)
  })
}
