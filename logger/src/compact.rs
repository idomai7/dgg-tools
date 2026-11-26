//! Compact: merge each closed day's live files into one zstd archive.
//!
//! A day is closed once it ended (UTC) more than a grace period ago, so no
//! capture is still writing to it. Its live files, and its existing archive
//! if it was compacted before (a late `sync` can bring more files), are
//! merged record for record, byte for byte: sorted by receive time, keeping
//! the order of records with equal times. The only records dropped are
//! copies made by merging the same file twice (each distinct record is kept
//! as often as any one input has it), and torn tails left by crashes, which
//! can't ever be read.
//!
//! Inputs must be in the newest version, `journal dgg.0.3`, as the archive
//! is written: its header and records ending at RS LF.
//!
//! Each Kick channel's journal is compacted the same way, as a journal root
//! of its own: its records are self-contained (each `open` names its
//! endpoint), so they merge as dgg's do. Its inputs may be `journal
//! kick.0.0` or `0.1`, which only added a kind, and the archive is written
//! as `0.1`. An archive's header keeps the informational fields every input
//! shares: a Kick channel's `channel`, and its `endpoint` if there was only
//! one that day.
//!
//! The archive is written to a temporary file, fsynced, read back and
//! checked against what was meant to be written, and only then renamed
//! into place and recorded in its month's manifest. Live files are deleted
//! last, and only if they haven't grown since they were read; a file that
//! did is merged again next time. A crash at any point leaves the live files
//! or a verified archive (or both, which the next run merges again). The
//! root's [archives' lock](archive::lock) is held throughout, so a `sync`
//! taking a peer's archive waits.
//!
//! The backfill journal is compacted the same way, as a journal root of its
//! own. Backfill runs write to days that are already closed, so the files of
//! a run still going (holding its lock, see [`writing`]) are left alone, to
//! be merged once it's done.
//!
//! A day still open can be compacted early ([`compact_open_day`]), to tidy
//! up after an upgrade without waiting for the day to end: only the files
//! no capture may still write are merged, into an archive the rest join
//! once the day is closed, as for a late `sync`. A capture may still write
//! its instance's files while it holds its
//! [live lock](::logger_journal::layout::live_lock_file), or, for builds
//! from before the lock, while its status is fresh.

use {
  crate::{capture::Handoff, control, service::Root},
  ::common::fs::MissingOk,
  ::common::{
    atomic_file,
    time::{DAY_MS, day_start},
  },
  ::logger_journal::{
    archive::{self, Archive, valid_day},
    layout::{EXTENSION, archive_file, live_day_dir, live_dir},
    record::{END, Header, Variant, kick, split, v0},
    writer::{capturing, writing},
  },
  ::sha2::{Digest, Sha256},
  ::std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
  },
};

/// The zstd level archives are compressed at by default.
pub const DEFAULT_LEVEL: i32 = 19;

/// What compacting one day did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Day {
  pub day: String,
  /// Live files merged (and deleted, unless they grew meanwhile).
  pub files: u64,
  /// Live files left in place because they grew while being compacted.
  pub grown: u64,
  /// Live files left in place because a backfill run still writes them.
  pub busy: u64,
  /// Live files of a day still open left in place because a capture may
  /// still write them.
  pub capturing: u64,
  /// Whether an existing archive for the day was merged in.
  pub remerged: bool,
  /// Copies of records dropped because they came from merging the same
  /// file twice.
  pub duplicates: u64,
  /// Torn tails dropped.
  pub torn: u64,
  pub archive: Archive,
}

/// The days with live files in journal `root` that are closed at `now`
/// (unix ms): they ended at least `grace` ago. Oldest first.
pub fn closed_days(
  root: &Root,
  now: i64,
  grace: Duration,
) -> io::Result<Vec<String>> {
  let mut days = live_days(root)?;
  days.retain(|day| closed(day, now, grace));
  Ok(days)
}

/// The days with live files in journal `root` that are still open at `now`
/// (unix ms): not [closed](closed_days). Oldest first.
pub fn open_days(
  root: &Root,
  now: i64,
  grace: Duration,
) -> io::Result<Vec<String>> {
  let mut days = live_days(root)?;
  days.retain(|day| !closed(day, now, grace));
  Ok(days)
}

/// The days with live files in journal `root`, oldest first.
fn live_days(root: &Root) -> io::Result<Vec<String>> {
  let Some(entries) = fs::read_dir(live_dir(&root.dir)).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut days = Vec::new();
  for entry in entries {
    let entry = entry?;
    let day = entry.file_name().to_string_lossy().into_owned();
    if entry.file_type()?.is_dir() && valid_day(&day) {
      days.push(day);
    }
  }
  days.sort();
  Ok(days)
}

/// Whether `day` ended at least `grace` before `now` (unix ms).
fn closed(day: &str, now: i64, grace: Duration) -> bool {
  day_start(day)
    .is_some_and(|start| now >= start + DAY_MS + grace.as_millis() as i64)
}

/// One input's records, without their ends, in the newest version, and how
/// much of it was read.
struct Input {
  header: Header,
  records: Vec<Vec<u8>>,
  torn: bool,
  /// For live files: the path and how many bytes were read.
  live: Option<(PathBuf, u64)>,
}

fn invalid(path: &Path, why: &str) -> io::Error {
  io::Error::new(
    io::ErrorKind::InvalidData,
    format!("{}: {why}", path.display()),
  )
}

/// Splits a journal's bytes into its header and records, and whether it has
/// a torn tail.
fn parse(
  path: &Path,
  bytes: &[u8],
) -> io::Result<(Header, Vec<Vec<u8>>, bool)> {
  let Some(header_end) = bytes.iter().position(|&b| b == b'\n') else {
    return Err(invalid(path, "no complete header"));
  };
  let header = Header::parse(&bytes[..header_end])
    .ok_or_else(|| invalid(path, "not a journal"))?;
  let current = match header.variant {
    Variant::Dgg => header.version == v0::VERSION,
    // Every readable minor of Kick's major 0 only added kinds.
    Variant::Kick => header.readable(),
  };
  if !current {
    return Err(invalid(path, "journal of an older version"));
  }
  let (records, tail) = split(&bytes[header_end + 1..]);
  let records = records
    .into_iter()
    .filter(|record| !record.is_empty())
    .map(<[u8]>::to_vec)
    .collect();
  Ok((header, records, !tail.is_empty()))
}

/// The header an archive of `inputs` is written with: their variant, in its
/// newest version, with the informational fields they all share. Fails if
/// they're of different variants.
fn archive_header(inputs: &[Input]) -> io::Result<Header> {
  let first = &inputs[0].header;
  if inputs
    .iter()
    .any(|input| input.header.variant != first.variant)
  {
    let why = "the day's files are of different variants";
    return Err(io::Error::new(io::ErrorKind::InvalidData, why));
  }
  let version = match first.variant {
    Variant::Dgg => v0::VERSION,
    Variant::Kick => kick::v0::VERSION,
  };
  let mut header = Header::new(first.variant, version);
  header.info = first
    .info
    .iter()
    .filter(|field| {
      inputs.iter().all(|input| input.header.info.contains(field))
    })
    .cloned()
    .collect();
  Ok(header)
}

/// A record's receive time, the first field; unreadable ones sort last.
fn receive_time(record: &[u8]) -> i64 {
  let end = record
    .iter()
    .position(|&b| b == b' ')
    .unwrap_or(record.len());
  ::std::str::from_utf8(&record[..end])
    .ok()
    .and_then(|field| field.parse().ok())
    .unwrap_or(i64::MAX)
}

/// Whether a backfill run still writes the live file at `path`, in journal
/// root `dir`.
pub(crate) fn busy_file(dir: &Path, path: &Path) -> io::Result<bool> {
  held_by(path, |instance| writing(dir, instance))
}

/// Whether a capture may still write the live file at `path`, of a day
/// still open, in journal root `dir`: its instance holds its live lock
/// there, or has a status there written no longer ago at `now` (unix ms)
/// than a capture's status goes stale.
pub(crate) fn captured(dir: &Path, path: &Path, now: i64) -> io::Result<bool> {
  let stale_after = Handoff::default().stale_after.as_millis() as i64;
  held_by(path, |instance| {
    if capturing(dir, instance)? {
      return Ok(true);
    }
    let status = control::path(&control::dir(dir), instance);
    let Some(metadata) = fs::metadata(status).missing_ok()? else {
      return Ok(false);
    };
    let written = metadata
      .modified()?
      .duration_since(UNIX_EPOCH)
      .map_or(0, |since| since.as_millis() as i64);
    Ok(now - written <= stale_after)
  })
}

/// Whether `held` holds for the instance that wrote the live file at
/// `path`: its instance's, `<instance>.journal`, or `<instance>.<n>.journal`
/// after a change of format.
fn held_by(
  path: &Path,
  mut held: impl FnMut(&str) -> io::Result<bool>,
) -> io::Result<bool> {
  let stem = path.file_stem().unwrap().to_string_lossy();
  let numbered = stem
    .rsplit_once('.')
    .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
  if held(&stem)? {
    return Ok(true);
  }
  match numbered {
    Some((instance, _)) => held(instance),
    None => Ok(false),
  }
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compacts closed `day`'s live files (and existing archive) in journal
/// `root` into its archive, at zstd `level`. `None` if it has no live files
/// that aren't [busy](Day::busy).
pub fn compact_day(
  root: &Root,
  day: &str,
  level: i32,
) -> io::Result<Option<Day>> {
  compact(root, day, level, None)
}

/// Like [`compact_day`], but for a `day` still open at `now` (unix ms):
/// only the live files no capture may still write are merged. `None` if
/// there are none.
pub fn compact_open_day(
  root: &Root,
  day: &str,
  level: i32,
  now: i64,
) -> io::Result<Option<Day>> {
  compact(root, day, level, Some(now))
}

/// Compacts `day` in `root`, at `level`; `open` at `now` if it's still open.
fn compact(
  root: &Root,
  day: &str,
  level: i32,
  open: Option<i64>,
) -> io::Result<Option<Day>> {
  let journal_root = root.dir.as_path();
  let _lock = archive::lock(journal_root)?;
  let mut inputs = Vec::new();
  let archive_path = archive_file(journal_root, day);
  let remerged = archive_path.exists();
  if remerged {
    let bytes = ::zstd::decode_all(File::open(&archive_path)?)?;
    let (header, records, torn) = parse(&archive_path, &bytes)?;
    inputs.push(Input {
      header,
      records,
      torn,
      live: None,
    });
  }
  let (mut live, mut busy, mut capturing) = (Vec::new(), 0, 0);
  let entries = fs::read_dir(live_day_dir(journal_root, day)).missing_ok()?;
  for entry in entries.into_iter().flatten() {
    let path = entry?.path();
    if !path
      .extension()
      .is_some_and(|extension| extension == EXTENSION)
    {
      continue;
    }
    if busy_file(journal_root, &path)? {
      busy += 1;
    } else if let Some(now) = open
      && captured(journal_root, &path, now)?
    {
      capturing += 1;
    } else {
      live.push(path);
    }
  }
  if live.is_empty() {
    return Ok(None);
  }
  live.sort_by_cached_key(|path| root.path(path));
  for path in live {
    let bytes = fs::read(&path)?;
    let (header, records, torn) = parse(&path, &bytes)?;
    inputs.push(Input {
      header,
      records,
      torn,
      live: Some((path, bytes.len() as u64)),
    });
  }

  // Merge: by receive time, stably. A record repeated within one input is
  // real (dgg can send the same frame twice in a millisecond), so each
  // distinct record is kept as many times as the input with the most copies
  // has it; extra copies only come from merging the same file twice.
  let mut keep: HashMap<&[u8], usize> = HashMap::new();
  for input in &inputs {
    let mut counts: HashMap<&[u8], usize> = HashMap::new();
    for record in &input.records {
      *counts.entry(record).or_default() += 1;
    }
    for (record, count) in counts {
      let most = keep.entry(record).or_default();
      *most = (*most).max(count);
    }
  }
  let mut records: Vec<&[u8]> = inputs
    .iter()
    .flat_map(|input| input.records.iter().map(Vec::as_slice))
    .collect();
  records.sort_by_key(|record| receive_time(record));
  let total = records.len();
  records.retain(|record| {
    let left = keep.get_mut(record).unwrap();
    let kept = *left > 0;
    *left = left.saturating_sub(1);
    kept
  });
  let header = archive_header(&inputs)?;
  let mut journal = header.text().into_bytes();
  for record in &records {
    journal.extend_from_slice(record);
    journal.extend_from_slice(END);
  }

  // Write, verify, then put in place.
  let compressed = ::zstd::encode_all(journal.as_slice(), level)?;
  atomic_file::replace_with(&archive_path, |file| {
    file.write_all(&compressed)?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    let mut written = Vec::new();
    file.read_to_end(&mut written)?;
    if written != compressed
      || ::zstd::decode_all(written.as_slice())? != journal
    {
      let temporary = atomic_file::temporary(&archive_path);
      return Err(invalid(&temporary, "archive didn't read back as written"));
    }
    Ok(())
  })?;
  let archive = Archive {
    day: day.into(),
    compressed: compressed.len() as u64,
    bytes: journal.len() as u64,
    records: records.len() as u64,
    sha256: hex(&Sha256::digest(&compressed)),
  };
  archive::record(journal_root, header.variant, &archive)?;

  // Only now delete the live files, unless they grew meanwhile.
  let (mut files, mut grown) = (0, 0);
  for (path, read) in inputs.iter().filter_map(|input| input.live.as_ref()) {
    files += 1;
    if fs::metadata(path)?.len() == *read {
      fs::remove_file(path)?;
    } else {
      grown += 1;
    }
  }
  let _ = fs::remove_dir(live_day_dir(journal_root, day));
  Ok(Some(Day {
    day: day.into(),
    files,
    grown,
    busy,
    capturing,
    remerged,
    duplicates: (total - records.len()) as u64,
    torn: inputs.iter().filter(|input| input.torn).count() as u64,
    archive,
  }))
}
