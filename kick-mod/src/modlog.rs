//! The mod log, `data/kick-mod/modlog.jsonl`: every action taken through
//! kick-mod, one JSON object a line, appended as it's answered, refused
//! ones too.

use {
  crate::actions::Asked,
  ::serde::{Deserialize, Serialize},
  ::std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
  },
};

pub const DEFAULT_PATH: &str = "data/kick-mod/modlog.jsonl";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
  /// When Kick answered, in unix ms.
  pub at: i64,
  pub by: Moderator,
  pub broadcaster_user_id: u64,
  /// What, on whom, and why.
  #[serde(flatten)]
  pub asked: Asked,
  pub answer: Answer,
}

/// Who took the action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moderator {
  pub user_id: u64,
  pub name: Option<String>,
}

/// What Kick said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Answer {
  Done,
  /// With the HTTP status, if Kick answered, and Kick's message, or what
  /// went wrong.
  Refused {
    status: Option<u16>,
    error: String,
  },
}

/// Appends `entry` to the log at `path`, made if it isn't there.
pub fn append(path: &Path, entry: &Entry) -> io::Result<()> {
  if let Some(dir) = path.parent() {
    fs::create_dir_all(dir)?;
  }
  let mut line = ::serde_json::to_string(entry)?;
  line.push('\n');
  // One write, so lines from writers at once don't interleave.
  let mut file = OpenOptions::new().create(true).append(true).open(path)?;
  file.write_all(line.as_bytes())
}

/// The entries in `text`, a log's contents.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
  text
    .lines()
    .enumerate()
    .filter(|(_, line)| !line.trim().is_empty())
    .map(|(at, line)| {
      ::serde_json::from_str(line).map_err(|e| format!("line {}: {e}", at + 1))
    })
    .collect()
}
