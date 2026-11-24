//! Migrate: brings the journal and the index up to this build's formats, in
//! place, as a supervisor starts.
//!
//! There's nothing to migrate: every host is on this build's formats
//! (`journal dgg.0.3`, index schema 0.6), so it only checks. It logs what an
//! upgrade touches, so its log shows how one went:
//!
//! - first, the build, and the versions it writes;
//! - for each journal root (dgg's, the backfill journal's and each Kick
//!   channel's), a line for each kind of file in it, with its version: live
//!   files, archive manifests, `synced-archives` and capture statuses. A
//!   file at another version, which this build can't read, gets a line of
//!   its own, and is left as it was;
//! - the index's schema, which must be [`SCHEMA_VERSION`] (or there's none
//!   yet).
//!
//! How the captures hand over is theirs to say, in their own logs.
//!
//! The next format change that needs a migration fills one in.
//!
//! It holds `index.lock`, `compact.lock` and `sync.lock` in the data
//! directory while it runs, waiting for them as a supervisor does, so
//! nothing indexes, compacts or syncs meanwhile. A dry run changes nothing,
//! and takes no locks.

use {
  crate::{
    BUILD, control,
    index::{SCHEMA_VERSION, schema_version},
    taken,
  },
  ::common::fs::MissingOk,
  ::logger_journal::{
    Format, Header, Variant, Version, archive,
    layout::{archive_dir, backfill_root, channel_root, kick_root, live_files},
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

/// What a [`migrate`] did, or with a dry run would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
  /// What was left as it was, with why.
  pub failed: Vec<String>,
  /// The index's schema, if there's an index.
  pub index: Option<Version>,
  /// Files at a version this build can't read, left as they were.
  pub unreadable: Vec<PathBuf>,
}

/// Migrates `<data>/journal` and `<data>/index.sqlite` (see the [module
/// docs](self)), saying what it does to `say`. Fails only if it can't go on
/// at all; what it can't migrate is in the report.
pub fn migrate(
  data: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(&str),
) -> io::Result<Report> {
  say(&format!(
    "build {BUILD}: writes journal dgg.{}, journal kick.{}, index schema {}",
    dotted(v0::VERSION),
    dotted(kick::v0::VERSION),
    dotted(SCHEMA_VERSION)
  ));
  let _locks = if dry_run {
    Vec::new()
  } else {
    hold(data, say)?
  };
  let mut report = Report::default();
  let journal = data.join("journal");
  let mut roots = vec![
    ("journal".to_owned(), journal.clone(), Variant::Dgg),
    ("backfill".to_owned(), backfill_root(&journal), Variant::Dgg),
  ];
  for slug in dir_names(&kick_root(&journal))? {
    let root = channel_root(&kick_root(&journal), &slug);
    roots.push((format!("kick {slug}"), root, Variant::Kick));
  }
  for (name, root, variant) in &roots {
    check_root(name, root, *variant, &mut report, say)?;
  }

  match index_schema(&data.join("index.sqlite"))? {
    None => say("index: none yet"),
    Some(version) => {
      report.index = Some(version);
      if version == SCHEMA_VERSION {
        say(&format!(
          "index: schema {}, as it should be",
          dotted(version)
        ));
      } else {
        let why = format!(
          "schema {}, not {}; rebuild it",
          dotted(version),
          dotted(SCHEMA_VERSION)
        );
        say(&format!("index: {why}"));
        report.failed.push(format!("index: {why}"));
      }
    }
  }

  Ok(report)
}

/// Checks each kind of file under journal `root`, `name` in what it says.
fn check_root(
  name: &str,
  root: &Path,
  variant: Variant,
  report: &mut Report,
  say: &mut dyn FnMut(&str),
) -> io::Result<()> {
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
      live_files(root)?,
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
      taken::VERSION,
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
    say(&format!(
      "{name}: {count}, {}{}",
      current.line_as(format),
      match unreadable.len() {
        0 => String::new(),
        1 => "; 1 at another version, left as it was".to_owned(),
        n => format!("; {n} at another version, left as they were"),
      }
    ));
    for (file, found) in unreadable {
      let relative = file.strip_prefix(root).unwrap_or(file);
      let relative: Vec<_> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
      let found =
        found.map_or("no header".to_owned(), |header| header.line_as(format));
      say(&format!(
        "{name}: {} is {found}, which this build can't read",
        relative.join("/")
      ));
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
fn hold(data: &Path, say: &mut dyn FnMut(&str)) -> io::Result<Vec<File>> {
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
            say(&format!("waiting for {name}: another supervisor has it"));
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
