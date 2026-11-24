//! A command's configuration and secrets, as JSON, from a file or stdin:
//! never from a flag's value or an environment variable.
//!
//! Each command reads its own type, which should refuse fields it doesn't
//! know (`#[serde(deny_unknown_fields)]`). Secrets in it are [`Secret`]s,
//! which never print. Errors name the field that's missing or unknown, or
//! where a value of the wrong type is, but never quote a value.
//!
//! Since configs are written by hand, they're read as JSONC: they may have
//! comments, `// ...` to the end of the line and `/* ... */`, outside
//! strings, and a trailing comma after an object's or array's last value.

use {
  ::serde::{Deserialize, Deserializer, Serialize, Serializer},
  ::serde_json::error::Category,
  ::std::{
    fmt, fs,
    io::{self, Read},
    path::Path,
  },
  ::zeroize::Zeroizing,
};

/// The most read, from a file or stdin.
pub const MAX_SIZE: u64 = 1 << 20;

/// A token, password or key: its `Debug`, and its serialisation (for showing
/// a config), are `<secret>`; only [`Secret::expose`] gets its value. It's
/// zeroed when dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
  pub fn new(value: impl Into<String>) -> Self {
    Self(Zeroizing::new(value.into()))
  }

  /// The value, on purpose.
  pub fn expose(&self) -> &str {
    &self.0
  }
}

impl fmt::Debug for Secret {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.write_str("<secret>")
  }
}

impl Serialize for Secret {
  fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str("<secret>")
  }
}

impl<'de> Deserialize<'de> for Secret {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    String::deserialize(deserializer).map(Self::new)
  }
}

/// Where a config is read from.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
  File(&'a Path),
  Stdin,
}

impl fmt::Display for Source<'_> {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::File(path) => write!(f, "{}", path.display()),
      Self::Stdin => f.write_str("the config on stdin"),
    }
  }
}

/// Reads a `T` from `source`, all of it, up to [`MAX_SIZE`].
pub fn read<T: for<'de> Deserialize<'de>>(source: Source) -> Result<T, String> {
  let text = match source {
    Source::File(path) => fs::File::open(path).and_then(read_all),
    Source::Stdin => read_all(io::stdin().lock()),
  };
  let text = text.map_err(|e| format!("{source}: {e}"))?;
  parse(&text).map_err(|e| format!("{source}: {e}"))
}

fn read_all(from: impl Read) -> io::Result<Zeroizing<String>> {
  let mut text = Zeroizing::new(String::new());
  from.take(MAX_SIZE + 1).read_to_string(&mut text)?;
  match text.len() as u64 > MAX_SIZE {
    true => Err(io::Error::other(format!("over {MAX_SIZE} bytes"))),
    false => Ok(text),
  }
}

/// A `T` from `text`, JSONC.
pub fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, String> {
  ::serde_json::from_str(&as_json(text)).map_err(|e| describe(&e))
}

/// `text` as JSON: its comments and trailing commas as spaces, a byte for
/// a byte, and comments' line breaks kept, so errors' lines and columns are
/// still `text`'s.
fn as_json(text: &str) -> Zeroizing<String> {
  let mut json = without_comments(text);
  let trailing = trailing_commas(&json);
  for at in trailing {
    json.replace_range(at..at + 1, " ");
  }
  json
}

/// Where the commas are in `json` (without comments) that follow a value
/// and come right before a `}` or `]`. A comma that follows none, as in
/// `[,]` or `[1,,]`, is left to be refused.
fn trailing_commas(json: &str) -> Vec<usize> {
  let bytes = json.as_bytes();
  let mut found = Vec::new();
  let mut in_string = false;
  let mut escaped = false;
  // The last byte outside a string that isn't whitespace.
  let mut last = None;
  for (at, &b) in bytes.iter().enumerate() {
    if in_string {
      match (escaped, b) {
        (true, _) => escaped = false,
        (false, b'\\') => escaped = true,
        (false, b'"') => in_string = false,
        _ => {}
      }
      continue;
    }
    match b {
      b'"' => in_string = true,
      b',' if !matches!(last, Some(b'[' | b'{' | b',') | None) => {
        let next = bytes[at + 1..].iter().find(|b| !b.is_ascii_whitespace());
        if matches!(next, Some(b'}' | b']')) {
          found.push(at);
        }
      }
      _ => {}
    }
    if !b.is_ascii_whitespace() {
      last = Some(b);
    }
  }
  found
}

/// `text` with its comments as spaces, a byte for a byte, and their line
/// breaks kept.
fn without_comments(text: &str) -> Zeroizing<String> {
  #[derive(Clone, Copy, PartialEq)]
  enum In {
    Json,
    String,
    Escape,
    Line,
    Block,
  }
  let blank = |out: &mut String, c: char| match c {
    '\n' | '\r' => out.push(c),
    c => out.extend(::std::iter::repeat_n(' ', c.len_utf8())),
  };
  let mut out = Zeroizing::new(String::with_capacity(text.len()));
  let mut chars = text.chars().peekable();
  let mut at = In::Json;
  while let Some(c) = chars.next() {
    match (at, c, chars.peek()) {
      (In::Json, '/', Some('/')) => at = In::Line,
      (In::Json, '/', Some('*')) => {
        chars.next();
        out.push_str("  ");
        at = In::Block;
        continue;
      }
      (In::Json, '"', _) => at = In::String,
      (In::String, '\\', _) => at = In::Escape,
      (In::String, '"', _) => at = In::Json,
      (In::Escape, ..) => at = In::String,
      (In::Line, '\n', _) => at = In::Json,
      (In::Block, '*', Some('/')) => {
        chars.next();
        out.push_str("  ");
        at = In::Json;
        continue;
      }
      _ => {}
    }
    match at {
      In::Line | In::Block => blank(&mut out, c),
      _ => out.push(c),
    }
  }
  out
}

/// What's wrong, without the value: serde's messages about a value's type
/// quote it.
fn describe(e: &::serde_json::Error) -> String {
  let message = e.to_string();
  let safe = ["missing field", "unknown field", "duplicate field"];
  match e.classify() {
    Category::Data if !safe.iter().any(|safe| message.starts_with(safe)) => {
      format!(
        "a value of the wrong type at line {} column {}",
        e.line(),
        e.column()
      )
    }
    _ => message,
  }
}
