//! Migrate: rewrites the journal's archives, and the backfill journal's,
//! from `journal 0.1`, a record per line, to `journal dgg.0.2`, records
//! ending at RS LF, and upgrades the index to match.
//!
//! Live files are left alone: compaction converts them into archives, with
//! the same checks.
//!
//! Each archive is [converted](convert::record) record by record into a new
//! file next to it, `<day>.new.journal.zst`, compressed at compaction's
//! level, so the same archive always gives the same bytes. That file is
//! fsynced, read back, and validated: read with [`Reader`], as the old one
//! is, each by its own framing, they must hold as many records, in the same
//! order, each decoding the same, or failing to the same error with the
//! same bytes, at offsets that map one to the other.
//!
//! A month at a time, once all its archives have been validated: the index
//! is upgraded for them, then each new file is renamed over its old one,
//! then the month's manifest is written (`journal-archive dgg.0.1`). A month
//! with a failure is left as it was and reported; the rest carry on.
//!
//! The index holds offsets into journal files in two tables: `files` (an
//! archive's checkpoint, with its SHA-256) and `unparsed` (where a record
//! that couldn't be indexed starts). In one transaction a month, each
//! archive's rows are mapped to the new file's offsets, if they're for the
//! old SHA-256; others are left, as the index reads those archives again
//! anyway. Once every month has been migrated, the index's schema goes from
//! 0.4 to 0.5, which only says its archive offsets are into `dgg.0.2` files.
//!
//! Rerunning it is safe. An archive already `dgg.0.2` is only checked
//! against its manifest entry, which is rewritten from the file if a crash
//! came between the rename and the manifest; and conversion is
//! deterministic, so an archive converted again after a crash between the
//! index's commit and the rename gives the SHA-256 the index was upgraded
//! for. Leftover new files are removed first.
//!
//! Each journal root's `control/synced-archives` (see
//! [`taken`](crate::taken)) from before headers is rewritten too, once what
//! the new one lists has been checked to be what the old one did.
//!
//! It holds `index.lock`, `compact.lock` and `sync.lock` in the data
//! directory while it runs, waiting for them as a supervisor does, so
//! nothing indexes, compacts or syncs meanwhile. A dry run converts and
//! validates into a temporary directory, replaces nothing, and takes no
//! locks.

use {
  crate::{
    compact::DEFAULT_LEVEL,
    index::{SCHEMA_VERSION, schema_version, user_version},
    taken,
  },
  ::common::{atomic_file, fs::MissingOk},
  ::logger_journal::{
    Header, ReadError, Reader, Variant, Version,
    archive::{self, Archive, MANIFEST},
    convert,
    layout::{ARCHIVE_SUFFIX, archive_dir, archive_file, backfill_root},
    record::{Framing, RS},
  },
  ::rusqlite::{Connection, OpenFlags, OptionalExtension, params},
  ::sha2::{Digest, Sha256},
  ::std::{
    collections::HashMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
  },
};

/// The index schema this upgrades from; it upgrades to [`SCHEMA_VERSION`].
pub const FROM_SCHEMA: Version = Version { major: 0, minor: 4 };

/// The locks in the data directory it holds, a supervisor's.
pub const LOCKS: [&str; 3] = ["index.lock", "compact.lock", "sync.lock"];

/// What a [`migrate`] did, or with a dry run would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
  /// Archives converted.
  pub converted: u64,
  /// Manifests written.
  pub manifests: u64,
  /// `synced-archives` files rewritten.
  pub taken: u64,
  /// Months left as they were, with why.
  pub failed: Vec<String>,
  /// The index's schema before, if there's one with a known schema.
  pub index: Option<Version>,
}

/// Migrates `<data>/journal` and its backfill journal, and
/// `<data>/index.sqlite` (see the [module docs](self)), saying what it does
/// to `say`. Fails only if it can't go on at all; a month that fails is in
/// the report.
pub fn migrate(
  data: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(&str),
) -> io::Result<Report> {
  let _locks = if dry_run {
    Vec::new()
  } else {
    hold(data, say)?
  };
  let mut report = Report::default();
  let mut index = open_index(&data.join("index.sqlite"), dry_run, say)?;
  report.index = index.as_ref().map(|(_, version)| *version);
  let scratch = if dry_run {
    let dir = ::std::env::temp_dir()
      .join(format!("dgg-logger-migrate-{}", ::std::process::id()));
    fs::create_dir_all(&dir)?;
    Some(dir)
  } else {
    None
  };
  let journal = data.join("journal");
  for (root, prefix) in [
    (journal.clone(), ""),
    (backfill_root(&journal), "backfill/"),
  ] {
    let name = format!("{prefix}{}", taken::PATH);
    match migrate_taken(&root, dry_run) {
      Ok(false) => {}
      Ok(true) => {
        report.taken += 1;
        say(&format!("{name}: rewritten"));
      }
      Err(why) => {
        say(&format!("{name}: left as it was: {why}"));
        report.failed.push(format!("{name}: {why}"));
      }
    }
    for month in months(&root)? {
      let at = Month {
        root: &root,
        prefix,
        dir: &month,
        scratch: scratch.as_deref(),
      };
      let name = at.name();
      let db = index.as_mut().map(|(db, _)| db);
      match at.migrate(db) {
        Ok(done) => {
          report.converted += done.converted;
          report.manifests += u64::from(done.manifest);
          if done.converted > 0 || done.manifest {
            say(&format!(
              "{name}: {} archives converted{}",
              done.converted,
              if done.manifest {
                ", manifest written"
              } else {
                ""
              }
            ));
          }
        }
        Err(why) => {
          say(&format!("{name}: left as it was: {why}"));
          report.failed.push(format!("{name}: {why}"));
        }
      }
    }
  }
  if let Some(dir) = scratch {
    let _ = fs::remove_dir_all(dir);
  }
  if let Some((db, version)) = &index
    && *version == FROM_SCHEMA
    && report.failed.is_empty()
    && !dry_run
  {
    db.pragma_update(None, "user_version", user_version(SCHEMA_VERSION))
      .map_err(io::Error::other)?;
    say(&format!(
      "index: schema {}.{} now",
      SCHEMA_VERSION.major, SCHEMA_VERSION.minor
    ));
  }
  Ok(report)
}

/// Rewrites journal `root`'s `synced-archives` from before headers, if it
/// is, once the new contents are checked to list the same; whether it was
/// (or with a dry run, would be).
fn migrate_taken(root: &Path, dry_run: bool) -> Result<bool, String> {
  let path = root.join(taken::PATH);
  let Some(text) = fs::read_to_string(&path)
    .missing_ok()
    .map_err(|e| e.to_string())?
  else {
    return Ok(false);
  };
  let (version, listed) = taken::decode(&text).ok_or("not readable")?;
  if version != taken::LEGACY {
    return Ok(false);
  }
  // Every line an entry, so none is dropped unnoticed.
  if listed.len() != text.lines().count() {
    return Err("a line that isn't `<day> <sha-256>`".into());
  }
  let new = taken::encode(&listed);
  if taken::decode(&new) != Some((taken::VERSION, listed)) {
    return Err("would read differently rewritten".into());
  }
  if !dry_run {
    atomic_file::replace(&path, new).map_err(|e| e.to_string())?;
  }
  Ok(true)
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

/// The index at `path`, if there is one with a schema this upgrades, and
/// that schema.
fn open_index(
  path: &Path,
  dry_run: bool,
  say: &mut dyn FnMut(&str),
) -> io::Result<Option<(Connection, Version)>> {
  if !path.exists() {
    return Ok(None);
  }
  let flags = if dry_run {
    OpenFlags::SQLITE_OPEN_READ_ONLY
  } else {
    OpenFlags::SQLITE_OPEN_READ_WRITE
  };
  let db =
    Connection::open_with_flags(path, flags).map_err(io::Error::other)?;
  db.busy_timeout(Duration::from_secs(10))
    .map_err(io::Error::other)?;
  let version: i64 = db
    .pragma_query_value(None, "user_version", |row| row.get(0))
    .map_err(io::Error::other)?;
  let version = schema_version(version);
  if version != FROM_SCHEMA && version != SCHEMA_VERSION {
    say(&format!(
      "index: schema {}.{}, left alone; rebuild it",
      version.major, version.minor
    ));
    return Ok(None);
  }
  Ok(Some((db, version)))
}

/// The month directories under `root`'s archive, oldest first.
fn months(root: &Path) -> io::Result<Vec<PathBuf>> {
  let mut months = Vec::new();
  for year in dirs(&archive_dir(root))? {
    months.extend(dirs(&year)?);
  }
  Ok(months)
}

fn dirs(path: &Path) -> io::Result<Vec<PathBuf>> {
  let Some(entries) = fs::read_dir(path).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut dirs = Vec::new();
  for entry in entries {
    let entry = entry?;
    if entry.file_type()?.is_dir() {
      dirs.push(entry.path());
    }
  }
  dirs.sort();
  Ok(dirs)
}

/// Each offset in an old file that's a record's boundary, and the new
/// file's.
type Offsets = HashMap<u64, u64>;

/// One month of one journal's archives.
struct Month<'a> {
  root: &'a Path,
  /// `""`, or `"backfill/"`: where `root` is in the journal.
  prefix: &'a str,
  dir: &'a Path,
  /// For a dry run, where new files go instead.
  scratch: Option<&'a Path>,
}

/// What migrating a month did.
struct Done {
  converted: u64,
  manifest: bool,
}

/// An archive converted and validated, not yet in place.
struct Converted {
  day: String,
  /// The new file.
  candidate: PathBuf,
  /// The old file's SHA-256, as its manifest entry gave it.
  old_sha256: String,
  new_sha256: String,
  offsets: Offsets,
}

impl Month<'_> {
  /// `archive/YYYY/MM`, under its prefix.
  fn name(&self) -> String {
    let relative = self.dir.strip_prefix(self.root).unwrap_or(self.dir);
    let relative = relative.to_string_lossy().replace('\\', "/");
    format!("{}{relative}", self.prefix)
  }

  fn migrate(&self, index: Option<&mut Connection>) -> Result<Done, String> {
    let error = |e: io::Error| e.to_string();
    let manifest_path = self.dir.join(MANIFEST);
    let Some(text) = fs::read_to_string(&manifest_path)
      .missing_ok()
      .map_err(error)?
    else {
      return Ok(Done {
        converted: 0,
        manifest: false,
      });
    };
    let first = text.lines().next().unwrap_or_default();
    let header = Header::parse_as(archive::FORMAT, first.as_bytes())
      .ok_or("its manifest isn't readable")?;
    let (variant, mut entries) =
      archive::decode(&text).ok_or("its manifest isn't readable")?;
    if variant != Variant::Dgg {
      return Err("its manifest isn't dgg's".into());
    }
    let mut changed = header.version != archive::version(Variant::Dgg);
    if self.scratch.is_none() {
      self.remove_leftovers().map_err(error)?;
    }
    let mut converted = Vec::new();
    let result = self.convert(&mut entries, &mut converted, &mut changed);
    let result = result.and_then(|()| match index {
      Some(db) => self.upgrade_index(db, &converted),
      None => Ok(()),
    });
    if let Err(why) = result {
      for done in &converted {
        let _ = fs::remove_file(&done.candidate);
      }
      return Err(why);
    }
    if self.scratch.is_some() {
      for done in &converted {
        let _ = fs::remove_file(&done.candidate);
      }
    } else {
      for done in &converted {
        let path = archive_file(self.root, &done.day);
        atomic_file::rename(&done.candidate, &path).map_err(error)?;
      }
      if changed {
        let text = archive::encode(Variant::Dgg, &entries);
        atomic_file::replace(&manifest_path, text).map_err(error)?;
      }
    }
    Ok(Done {
      converted: converted.len() as u64,
      manifest: changed,
    })
  }

  /// New files a crash left behind.
  fn remove_leftovers(&self) -> io::Result<()> {
    for entry in fs::read_dir(self.dir)? {
      let path = entry?.path();
      let name = path.file_name().unwrap_or_default().to_string_lossy();
      if name.ends_with(&format!(".new{ARCHIVE_SUFFIX}")) {
        fs::remove_file(&path)?;
      }
    }
    Ok(())
  }

  /// Converts and validates each of `entries`' archives still of a record
  /// per line into `converted`, updating its entry; checks the others'
  /// entries against their files. `changed` if any entry did.
  fn convert(
    &self,
    entries: &mut [Archive],
    converted: &mut Vec<Converted>,
    changed: &mut bool,
  ) -> Result<(), String> {
    for entry in entries {
      let path = entry.path(self.root);
      let day = entry.day.clone();
      let failed = |why: String| format!("{day}: {why}");
      let compressed = fs::read(&path).map_err(|e| failed(e.to_string()))?;
      let bytes = ::zstd::decode_all(compressed.as_slice())
        .map_err(|e| failed(e.to_string()))?;
      let header_end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| failed("no complete header".into()))?;
      let header = Header::parse(&bytes[..header_end])
        .filter(|header| header.variant == Variant::Dgg && header.readable())
        .ok_or_else(|| failed("not a readable dgg journal".into()))?;
      let body = &bytes[header_end + 1..];
      if header.framing() == Framing::RsLf {
        let (records, _) = Framing::RsLf.split(body);
        let found = Archive {
          day: day.clone(),
          compressed: compressed.len() as u64,
          bytes: bytes.len() as u64,
          records: records.len() as u64,
          sha256: hex(&Sha256::digest(&compressed)),
        };
        if found != *entry {
          *entry = found;
          *changed = true;
        }
        continue;
      }
      let candidate = match self.scratch {
        Some(dir) => dir.join(format!("{day}.new{ARCHIVE_SUFFIX}")),
        None => self.dir.join(format!("{day}.new{ARCHIVE_SUFFIX}")),
      };
      let (new, records, offsets) =
        convert_archive(&header, header_end + 1, &bytes).map_err(failed)?;
      let new_compressed = ::zstd::encode_all(new.as_slice(), DEFAULT_LEVEL)
        .map_err(|e| failed(e.to_string()))?;
      let new_sha256 = hex(&Sha256::digest(&new_compressed));
      // Pushed first, so a failure from here on removes it.
      converted.push(Converted {
        day: day.clone(),
        candidate: candidate.clone(),
        old_sha256: entry.sha256.clone(),
        new_sha256: new_sha256.clone(),
        offsets,
      });
      write_checked(&candidate, &new_compressed)
        .map_err(|e| failed(e.to_string()))?;
      let offsets = &converted.last().unwrap().offsets;
      validate(&path, &candidate, offsets, &bytes, &new).map_err(failed)?;
      *entry = Archive {
        day,
        compressed: new_compressed.len() as u64,
        bytes: new.len() as u64,
        records,
        sha256: new_sha256,
      };
      *changed = true;
    }
    Ok(())
  }

  /// Maps the index's offsets into each of `converted`'s old files to the
  /// new file's, in one transaction; with a dry run, only checks they map.
  fn upgrade_index(
    &self,
    db: &mut Connection,
    converted: &[Converted],
  ) -> Result<(), String> {
    let sql = |e: ::rusqlite::Error| format!("index: {e}");
    let dry_run = self.scratch.is_some();
    let transaction = db.transaction().map_err(sql)?;
    for done in converted {
      let day = &done.day;
      let path = format!(
        "{}archive/{}/{}/{day}{ARCHIVE_SUFFIX}",
        self.prefix,
        &day[..4],
        &day[5..7]
      );
      let row: Option<(i64, Option<String>)> = transaction
        .query_row(
          "SELECT offset, digest FROM files WHERE path = ?1",
          [&path],
          |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql)?;
      let Some((offset, Some(digest))) = row else {
        continue;
      };
      if digest != done.old_sha256 {
        continue;
      }
      let map = |offset: i64| {
        done.offsets.get(&(offset as u64)).copied().ok_or_else(|| {
          format!("{day}: the index's offset {offset} isn't a record's")
        })
      };
      let new_offset = map(offset)?;
      let mut query = transaction
        .prepare("SELECT offset, item FROM unparsed WHERE path = ?1")
        .map_err(sql)?;
      let unparsed: Vec<(i64, i64)> = query
        .query_map([&path], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
      drop(query);
      let unparsed = unparsed
        .into_iter()
        .map(|(offset, item)| Ok((offset, item, map(offset)?)))
        .collect::<Result<Vec<_>, String>>()?;
      if dry_run {
        continue;
      }
      transaction
        .execute(
          "UPDATE files SET offset = ?1, digest = ?2 WHERE path = ?3",
          params![new_offset as i64, done.new_sha256, path],
        )
        .map_err(sql)?;
      // Through negative offsets, so no two rows ever share one.
      for (offset, item, new) in unparsed {
        transaction
          .execute(
            "UPDATE unparsed SET offset = ?1
             WHERE path = ?2 AND offset = ?3 AND item = ?4",
            params![-1 - new as i64, path, offset, item],
          )
          .map_err(sql)?;
      }
      transaction
        .execute(
          "UPDATE unparsed SET offset = -1 - offset
           WHERE path = ?1 AND offset < 0",
          [&path],
        )
        .map_err(sql)?;
    }
    transaction.commit().map_err(sql)
  }
}

/// The journal `bytes`, of a record per line after its `header`, which ends
/// at `body`, converted: the new journal, its records, and each record
/// boundary's offset in the old mapped to the new's.
fn convert_archive(
  header: &Header,
  body: usize,
  bytes: &[u8],
) -> Result<(Vec<u8>, u64, Offsets), String> {
  let (records, tail) = Framing::Lf.split(&bytes[body..]);
  if !tail.is_empty() {
    return Err("a torn record at its end".into());
  }
  let newest = convert::newest(Variant::Dgg);
  let mut new = format!("{}\n", newest.line()).into_bytes();
  let mut offsets = HashMap::from([(0, 0), (body as u64, new.len() as u64)]);
  let (mut old_at, mut count) = (body, 0);
  for record in records {
    old_at += record.len() + 1;
    if !record.is_empty() {
      let converted =
        convert::record(header, record).map_err(|e| e.to_string())?;
      new.extend_from_slice(&converted);
      new.extend_from_slice(newest.framing().end());
      count += 1;
    }
    offsets.insert(old_at as u64, new.len() as u64);
  }
  Ok((new, count, offsets))
}

/// Writes `bytes` to `path`, fsyncs it, and reads it back.
fn write_checked(path: &Path, bytes: &[u8]) -> io::Result<()> {
  let mut file = OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .open(path)?;
  file.write_all(bytes)?;
  file.sync_all()?;
  drop(file);
  if fs::read(path)? != bytes {
    return Err(io::Error::other("didn't read back as written"));
  }
  Ok(())
}

/// Whether the archives at `old` and `new` read the same, record by record,
/// at offsets `offsets` maps; `old_bytes` and `new_bytes` are their
/// journals.
fn validate(
  old: &Path,
  new: &Path,
  offsets: &Offsets,
  old_bytes: &[u8],
  new_bytes: &[u8],
) -> Result<(), String> {
  let read = |path: &Path| -> Result<Vec<_>, String> {
    Ok(
      Reader::open(path, 0)
        .map_err(|e| format!("reading it: {e:?}"))?
        .collect(),
    )
  };
  let (old, new) = (read(old)?, read(new)?);
  if old.len() != new.len() {
    return Err(format!("{} records read back as {}", old.len(), new.len()));
  }
  let maps = |from: u64, to: u64| offsets.get(&from) == Some(&to);
  for (old, new) in old.iter().zip(&new) {
    let same = match (old, new) {
      (Ok(old), Ok(new)) => {
        old.record == new.record
          && maps(old.offset, new.offset)
          && maps(old.next, new.next)
      }
      (
        Err(ReadError::Record {
          offset: old_at,
          error: old_error,
        }),
        Err(ReadError::Record {
          offset: new_at,
          error: new_error,
        }),
      ) => {
        old_error == new_error
          && maps(*old_at, *new_at)
          && record_at(old_bytes, *old_at, b"\n")
            == record_at(new_bytes, *new_at, &[RS, b'\n'])
      }
      _ => false,
    };
    if !same {
      let at = match old {
        Ok(entry) => entry.offset,
        Err(ReadError::Record { offset, .. }) => *offset,
        Err(_) => 0,
      };
      return Err(format!("the record at {at} reads differently converted"));
    }
  }
  Ok(())
}

/// The record at `at` in `bytes`, up to `end`.
fn record_at<'a>(bytes: &'a [u8], at: u64, end: &[u8]) -> &'a [u8] {
  let rest = &bytes[at as usize..];
  let length = rest
    .windows(end.len())
    .position(|window| window == end)
    .unwrap_or(rest.len());
  &rest[..length]
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}
