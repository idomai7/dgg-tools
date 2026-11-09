//! Status files capture instances use to hand off to each other.
//!
//! Each running instance keeps `<journal root>/control/<instance>.status`
//! up to date:
//!
//! ```text
//! capture-status 0.0
//! instance <id>
//! started <unix ms>
//! heartbeat <unix ms>
//! healthy-since <unix ms>
//! build <version>+<commit>
//! ```
//!
//! The first line is the format name and `<major>.<minor>` version. Each
//! later line is `<key> <value>`. `healthy-since` is omitted while the
//! instance isn't healthy; `build` (the [`BUILD`](crate::BUILD) it runs) by
//! instances from before it was added. It came without a version bump:
//! readers have always skipped keys they don't know, and old instances must
//! still read new ones' statuses to hand off to them.
//!
//! While the major is 0 (pre-release) any minor may break, so readers accept
//! only the exact header they write. From 1.0 on, readers will ignore keys
//! they don't recognise, so a minor bump may add keys; anything else will be
//! a major bump.
//!
//! An instance on another host makes itself known by pushing its status to
//! this host's `serve` (see [`push_body`]), which writes it here with its
//! times shifted onto this host's clock, so local instances treat it like
//! any other.

use {
  ::logger_journal::record::v0::valid_instance,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
  },
};

/// The header line this crate writes and reads.
pub const HEADER: &str = "capture-status 0.0";

/// File extension of status files.
pub const EXTENSION: &str = "status";

/// One instance's status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
  pub instance: String,
  /// When the instance started, unix ms.
  pub started: i64,
  /// When the status was last written, unix ms.
  pub heartbeat: i64,
  /// Since when the instance has been healthy, unix ms, if it is.
  pub healthy_since: Option<i64>,
  /// The build it runs, if it says.
  pub build: Option<String>,
}

impl Status {
  /// The file contents.
  pub fn encode(&self) -> String {
    let mut text = format!(
      "{HEADER}\ninstance {}\nstarted {}\nheartbeat {}\n",
      self.instance, self.started, self.heartbeat
    );
    if let Some(since) = self.healthy_since {
      text += &format!("healthy-since {since}\n");
    }
    if let Some(build) = &self.build {
      text += &format!("build {build}\n");
    }
    text
  }

  /// Parses file contents, or `None` if they aren't a readable status.
  pub fn decode(text: &str) -> Option<Self> {
    let mut lines = text.strip_suffix('\n')?.split('\n');
    if lines.next()? != HEADER {
      return None;
    }
    let (mut instance, mut started, mut heartbeat, mut healthy_since) =
      (None, None, None, None);
    let mut build = None;
    for line in lines {
      let (key, value) = line.split_once(' ')?;
      let slot = match key {
        "instance" => {
          if instance.replace(value).is_some() {
            return None;
          }
          continue;
        }
        "build" => {
          if value.is_empty() || build.replace(value).is_some() {
            return None;
          }
          continue;
        }
        "started" => &mut started,
        "heartbeat" => &mut heartbeat,
        "healthy-since" => &mut healthy_since,
        _ => continue,
      };
      if !digits(value) || slot.replace(value.parse().ok()?).is_some() {
        return None;
      }
    }
    let instance = instance?;
    if !valid_instance(instance) {
      return None;
    }
    Some(Self {
      instance: instance.into(),
      started: started?,
      heartbeat: heartbeat?,
      healthy_since,
      build: build.map(str::to_owned),
    })
  }

  /// Moves every time by `by` ms, onto another clock.
  pub fn shift(&mut self, by: i64) {
    self.started += by;
    self.heartbeat += by;
    if let Some(since) = self.healthy_since.as_mut() {
      *since += by;
    }
  }
}

/// The body pushed to another host's `serve`: `now <sender's unix ms>`,
/// then the status file as written.
pub fn push_body(status: &str, now: i64) -> String {
  format!("now {now}\n{status}")
}

/// Decodes a pushed body, shifting its times by the difference between the
/// receiver's clock (`now`) and the sender's.
pub fn parse_push(body: &str, now: i64) -> Option<Status> {
  let (first, status) = body.split_once('\n')?;
  let sent = first.strip_prefix("now ")?;
  if !digits(sent) {
    return None;
  }
  let mut status = Status::decode(status)?;
  status.shift(now - sent.parse::<i64>().ok()?);
  Some(status)
}

fn digits(text: &str) -> bool {
  !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The control directory under a journal root.
pub fn dir(journal_root: &Path) -> PathBuf {
  journal_root.join("control")
}

/// One instance's status file.
pub fn path(dir: &Path, instance: &str) -> PathBuf {
  dir.join(format!("{instance}.{EXTENSION}"))
}

/// Writes `status` to its file, atomically: to a temporary file first, then
/// renamed over the old one, so readers never see a partial status.
pub fn write(dir: &Path, status: &Status) -> io::Result<()> {
  fs::create_dir_all(dir)?;
  let path = path(dir, &status.instance);
  let temporary = path.with_extension(format!("{EXTENSION}.tmp"));
  fs::write(&temporary, status.encode())?;
  fs::rename(temporary, path)
}

/// Deletes an instance's status file, if it exists. Windows refuses while
/// something has the file open without allowing deletion (a virus scanner
/// or the search indexer, say), so a failure is retried for up to two
/// seconds; a status left behind would show as a stale capture.
pub fn remove(dir: &Path, instance: &str) -> io::Result<()> {
  let path = path(dir, instance);
  let until = Instant::now() + Duration::from_secs(2);
  loop {
    match fs::remove_file(&path) {
      Ok(()) => return Ok(()),
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
      Err(e) if Instant::now() >= until => return Err(e),
      Err(_) => thread::sleep(Duration::from_millis(50)),
    }
  }
}

/// Every readable status in `dir`. Files that can't be read or decoded (one
/// mid-rename, say) are skipped; they'll be read on a later round.
pub fn read_all(dir: &Path) -> io::Result<Vec<Status>> {
  let entries = match fs::read_dir(dir) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(e),
  };
  let mut statuses = Vec::new();
  for entry in entries {
    let path = entry?.path();
    if path
      .extension()
      .is_some_and(|extension| extension == EXTENSION)
      && let Ok(text) = fs::read_to_string(&path)
      && let Some(status) = Status::decode(&text)
    {
      statuses.push(status);
    }
  }
  Ok(statuses)
}

/// The instance `me` should hand off to, if any: one started after it (ties
/// broken by id) whose heartbeat is no older than `stale_after` and which
/// has been healthy for at least `grace`. The latest-started such instance
/// wins.
pub fn successor<'a>(
  me: &Status,
  others: &'a [Status],
  now: i64,
  grace: Duration,
  stale_after: Duration,
) -> Option<&'a Status> {
  let (grace, stale_after) = (grace.as_millis(), stale_after.as_millis());
  let age = |since: i64| u128::try_from(now - since).unwrap_or(0);
  others
    .iter()
    .filter(|other| {
      (other.started, &other.instance) > (me.started, &me.instance)
    })
    .filter(|other| age(other.heartbeat) <= stale_after)
    .filter(|other| other.healthy_since.is_some_and(|h| age(h) >= grace))
    .max_by_key(|other| (other.started, &other.instance))
}
