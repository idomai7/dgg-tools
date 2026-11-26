//! Migrate: brings the journal and the index up to this build's layout and
//! formats, in place, as a supervisor starts.
//!
//! Every host is on this build's formats (`journal dgg.0.3`, index schema
//! 0.6) and layout, dgg's journals by channel; a move of dgg's journals
//! while capture keeps running would be [`crate::dgg::migrate`]'s, and
//! there's none now. Kick's index of every channel,
//! [`OLD_FILE`](kick_index::OLD_FILE), is deleted, each channel's index
//! being rebuilt from its journal. It logs what an upgrade touches, so its
//! log shows how one went:
//!
//! - first, the build, and the versions it writes;
//! - anything left of an older layout (see [`OLD_LAYOUT`]), which nothing
//!   reads now;
//! - what moved, if a move is under way, and what was left where it was;
//! - for each service's journal roots (each dgg channel's, its backfill
//!   journal's and each Kick channel's), a line for each kind of file in it,
//!   with its
//!   version: live files, archive manifests, `synced-archives` and capture
//!   statuses. A file at another version, which this build can't read, gets
//!   a line of its own, and is left as it was;
//! - Kick's old index, deleted;
//! - each index's schema (see [`Service::indexes`]), which must be its
//!   service's (or there's none yet): [`SCHEMA_VERSION`] for each dgg
//!   channel's, [`SCHEMA_VERSION`](kick_index::SCHEMA_VERSION) for each
//!   Kick channel's.
//!
//! How the captures hand over is theirs to say, in their own logs.
//!
//! It holds `index.lock`, `compact.lock` and `sync.lock` in the data
//! directory while it runs, waiting for them as a supervisor does, so
//! nothing indexes, compacts or syncs meanwhile. A dry run changes nothing,
//! and takes no locks.

use {
  crate::{
    BUILD, control,
    dgg::{self, Dgg, index::SCHEMA_VERSION},
    index::{self, schema_version},
    kick::{Kick, index as kick_index},
    service::{Root, Service},
    taken,
  },
  ::common::{fs::MissingOk, log::Level},
  ::logger_journal::{
    Format, Header, Variant, Version, archive,
    layout::archive_dir,
    record::{JOURNAL, kick, v0},
  },
  ::rusqlite::{Connection, OpenFlags},
  ::std::{
    fs::{self, File, TryLockError},
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    thread,
    time::Duration,
  },
};

/// The locks in the data directory it holds, a supervisor's.
pub const LOCKS: [&str; 3] = ["index.lock", "compact.lock", "sync.lock"];

/// What older layouts had in the data directory, and this one doesn't:
/// before dgg's journals had a directory of their own, then before they
/// were by channel.
pub const OLD_LAYOUT: [&str; 10] = [
  "journal/live",
  "journal/archive",
  "journal/control",
  "journal/backfill",
  "index.sqlite",
  "journal/dgg/live",
  "journal/dgg/archive",
  "journal/dgg/control",
  "journal/dgg/backfill",
  "index-dgg.sqlite",
];

/// What a [`migrate`] did, or with a dry run would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
  /// What was left as it was, with why.
  pub failed: Vec<String>,
  /// What's left of the [old layout](OLD_LAYOUT), which nothing reads now.
  pub old: Vec<PathBuf>,
  /// Files moved, by a move under way.
  pub moved: u64,
  /// Live files left where they were, still being written.
  pub left: Vec<PathBuf>,
  /// Whether Kick's old index was deleted (or with a dry run, would be).
  pub kick_index_removed: bool,
  /// Each index there is, by its file, and its schema.
  pub indexes: Vec<(String, Version)>,
  /// Files at a version this build can't read, left as they were.
  pub unreadable: Vec<PathBuf>,
}

/// Migrates `<data>/journal` and the indexes in `<data>` (see the [module
/// docs](self)), saying what it does to `say`. Fails only if it can't go on
/// at all; what it can't migrate is in the report.
pub fn migrate(
  data: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<Report> {
  say(
    Level::Info,
    &format!(
      "build {BUILD}: writes journal dgg.{}, journal kick.{}, index schema \
       {}, Kick's index schema {}",
      dotted(v0::VERSION),
      dotted(kick::v0::VERSION),
      dotted(SCHEMA_VERSION),
      dotted(kick_index::SCHEMA_VERSION)
    ),
  );
  let _locks = if dry_run {
    Vec::new()
  } else {
    hold(data, say)?
  };
  let mut report = Report {
    old: old_layout(data, say),
    ..Report::default()
  };
  let moved = dgg::migrate::move_in(data, dry_run, say)?;
  report.moved = moved.files;
  report.left = moved.left;
  report.failed = moved.failed;
  let journal = data.join("journal");
  check_service::<Dgg>(&journal, &mut report, say)?;
  check_service::<Kick>(&journal, &mut report, say)?;

  remove_old_kick_index(data, dry_run, &mut report, say);
  check_indexes::<Dgg>(data, SCHEMA_VERSION, &mut report, say)?;
  check_indexes::<Kick>(data, kick_index::SCHEMA_VERSION, &mut report, say)?;

  Ok(report)
}

/// Deletes Kick's index of every channel, in `data`, if it's there: each
/// channel's has its own now, rebuilt from its journal.
fn remove_old_kick_index(
  data: &Path,
  dry_run: bool,
  report: &mut Report,
  say: &mut dyn FnMut(Level, &str),
) {
  let name = kick_index::OLD_FILE;
  let path = data.join(name);
  if !path.exists() {
    return;
  }
  let removed = match dry_run {
    true => Ok(()),
    false => index::remove(&path),
  };
  match removed {
    Ok(()) => {
      report.kick_index_removed = true;
      say(
        Level::Info,
        &format!(
          "{name}: deleted, as each Kick channel's index is its own now, \
           rebuilt from its journal"
        ),
      );
    }
    Err(e) => {
      let why = format!("{name}: can't delete it: {e}");
      say(Level::Error, &why);
      report.failed.push(why);
    }
  }
}

/// Checks each of `S`'s indexes, in `data`, is at `wanted`.
fn check_indexes<S: Service>(
  data: &Path,
  wanted: Version,
  report: &mut Report,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<()> {
  for indexed in S::indexes(&S::dir(&data.join("journal")))? {
    let path = data.join(&indexed.file);
    let name = &indexed.file;
    if let Some(version) =
      check_index(name, &path, wanted, &mut report.failed, say)?
    {
      report.indexes.push((indexed.file, version));
    }
  }
  Ok(())
}

/// What's left in `data` of the [old layout](OLD_LAYOUT), each said.
fn old_layout(data: &Path, say: &mut dyn FnMut(Level, &str)) -> Vec<PathBuf> {
  let left: Vec<PathBuf> = OLD_LAYOUT
    .iter()
    .map(|old| data.join(old))
    .filter(|path| path.exists())
    .collect();
  for path in &left {
    say(
      Level::Warn,
      &format!(
        "{} is left from an older layout; nothing reads it now",
        path.display()
      ),
    );
  }
  left
}

/// Checks the index at `path`, `name` in what it says, is at `wanted`, if
/// there is one; its schema. One that isn't is added to `failed`.
fn check_index(
  name: &str,
  path: &Path,
  wanted: Version,
  failed: &mut Vec<String>,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<Option<Version>> {
  let Some(version) = index_schema(path)? else {
    say(Level::Info, &format!("{name}: none yet"));
    return Ok(None);
  };
  if version == wanted {
    say(
      Level::Info,
      &format!("{name}: schema {}, as it should be", dotted(version)),
    );
  } else {
    let why = format!(
      "schema {}, not {}; rebuild it",
      dotted(version),
      dotted(wanted)
    );
    say(Level::Error, &format!("{name}: {why}"));
    failed.push(format!("{name}: {why}"));
  }
  Ok(Some(version))
}

/// Checks each of `S`'s journal roots, in the journal directory `journal`.
fn check_service<S: Service>(
  journal: &Path,
  report: &mut Report,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<()> {
  for root in S::roots(&S::dir(journal))? {
    check_root(&root, S::VARIANT, report, say)?;
  }
  Ok(())
}

/// Checks each kind of file in journal `root`.
fn check_root(
  journal_root: &Root,
  variant: Variant,
  report: &mut Report,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<()> {
  let (name, root) = (&journal_root.name, journal_root.dir.as_path());
  let journal = match variant {
    Variant::Dgg => v0::VERSION,
    Variant::Kick => kick::v0::VERSION,
  };
  let status = match variant {
    Variant::Dgg => control::VERSION,
    Variant::Kick => control::KICK_VERSION,
  };
  let mut manifests = Vec::new();
  for year in dir_names(&archive_dir(root))? {
    for month in dir_names(&archive_dir(root).join(&year))? {
      let path = archive_dir(root).join(&year).join(month);
      let path = path.join(archive::MANIFEST);
      if path.exists() {
        manifests.push(path);
      }
    }
  }
  let synced = root.join(taken::PATH);
  let synced: Vec<PathBuf> = match synced.exists() {
    true => vec![synced],
    false => Vec::new(),
  };
  let statuses = files_with(&control::dir(root), control::EXTENSION)?;
  // Each kind, one and many of it, its files, and their format and version.
  let kinds = [
    (
      ["live file", "live files"],
      journal_root.live_files()?,
      JOURNAL,
      journal,
    ),
    (
      ["archive manifest", "archive manifests"],
      manifests,
      archive::FORMAT,
      archive::version(variant),
    ),
    (
      ["synced-archives"; 2],
      synced,
      taken::FORMAT,
      taken::version(variant),
    ),
    (
      ["capture status", "capture statuses"],
      statuses,
      control::FORMAT,
      status,
    ),
  ];
  for ([one, many], files, format, version) in kinds {
    if files.is_empty() {
      continue;
    }
    let current = Header::new(variant, version);
    let mut unreadable = Vec::new();
    for file in &files {
      match header(file, format)? {
        Some(header)
          if header.variant == variant && header.version == version => {}
        found => unreadable.push((file, found)),
      }
    }
    let count = match files.len() {
      1 if one == many => one.to_owned(),
      1 => format!("1 {one}"),
      n => format!("{n} {many}"),
    };
    say(
      Level::Info,
      &format!(
        "{name}: {count}, {}{}",
        current.line_as(format),
        match unreadable.len() {
          0 => String::new(),
          1 => "; 1 at another version, left as it was".to_owned(),
          n => format!("; {n} at another version, left as they were"),
        }
      ),
    );
    for (file, found) in unreadable {
      let path = journal_root.path(file);
      let relative = &path[journal_root.prefix.len()..];
      let found =
        found.map_or("no header".to_owned(), |header| header.line_as(format));
      say(
        Level::Warn,
        &format!("{name}: {relative} is {found}, which this build can't read"),
      );
      report.unreadable.push(file.clone());
    }
  }
  Ok(())
}

/// The header of the file at `path`, in `format`, if it has one.
fn header(path: &Path, format: Format) -> io::Result<Option<Header>> {
  let Some(file) = File::open(path).missing_ok()? else {
    return Ok(None);
  };
  let mut line = Vec::new();
  BufReader::new(file.take(4096)).read_until(b'\n', &mut line)?;
  Ok(
    line
      .strip_suffix(b"\n")
      .and_then(|line| Header::parse_as(format, line)),
  )
}

/// The names of the directories in `dir`, sorted.
fn dir_names(dir: &Path) -> io::Result<Vec<String>> {
  let Some(entries) = fs::read_dir(dir).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut names = Vec::new();
  for entry in entries {
    let entry = entry?;
    if entry.file_type()?.is_dir() {
      names.push(entry.file_name().to_string_lossy().into_owned());
    }
  }
  names.sort();
  Ok(names)
}

/// The files in `dir` with `extension`, sorted.
fn files_with(dir: &Path, extension: &str) -> io::Result<Vec<PathBuf>> {
  let Some(entries) = fs::read_dir(dir).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut files = Vec::new();
  for entry in entries {
    let path = entry?.path();
    if path.extension().is_some_and(|found| found == extension) {
      files.push(path);
    }
  }
  files.sort();
  Ok(files)
}

/// `version` as `<major>.<minor>`.
fn dotted(version: Version) -> String {
  format!("{}.{}", version.major, version.minor)
}

/// Waits for each of [`LOCKS`] in `data`, and holds them.
fn hold(
  data: &Path,
  say: &mut dyn FnMut(Level, &str),
) -> io::Result<Vec<File>> {
  let mut held = Vec::new();
  for name in LOCKS {
    let path = data.join(name);
    let mut said = false;
    loop {
      let file = File::create(&path)?;
      match file.try_lock() {
        Ok(()) => {
          held.push(file);
          break;
        }
        Err(TryLockError::WouldBlock) => {
          if !said {
            say(
              Level::Warn,
              &format!("waiting for {name}: another supervisor has it"),
            );
            said = true;
          }
          thread::sleep(Duration::from_secs(1));
        }
        Err(TryLockError::Error(e)) => return Err(e),
      }
    }
  }
  Ok(held)
}

/// The schema of the index at `path`, if there is one.
fn index_schema(path: &Path) -> io::Result<Option<Version>> {
  if !path.exists() {
    return Ok(None);
  }
  let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    .map_err(io::Error::other)?;
  db.busy_timeout(Duration::from_secs(10))
    .map_err(io::Error::other)?;
  let version: i64 = db
    .pragma_query_value(None, "user_version", |row| row.get(0))
    .map_err(io::Error::other)?;
  Ok(Some(schema_version(version)))
}
