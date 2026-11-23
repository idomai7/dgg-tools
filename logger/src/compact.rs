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
//! The archive is written to a temporary file, fsynced, read back and
//! checked against what was meant to be written, and only then renamed
//! into place and recorded in its month's manifest. Live files are deleted
//! last, and only if they haven't grown since they were read; a file that
//! did is merged again next time. A crash at any point leaves the live files
//! or a verified archive (or both, which the next run merges again).
//!
//! The backfill journal is compacted the same way, as a journal root of its
//! own. Backfill runs write to days that are already closed, so the files of
//! a run still going (holding its lock, see [`writing`]) are left alone, to
//! be merged once it's done.

use {
  ::common::fs::MissingOk,
  ::common::{
    atomic_file,
    time::{DAY_MS, day_start},
  },
  ::logger_journal::{
    archive::{self, Archive, valid_day},
    layout::{EXTENSION, archive_file, live_day_dir, live_dir},
    record::{Header, Variant, v0},
    writer::writing,
  },
  ::sha2::{Digest, Sha256},
  ::std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
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
  /// Whether an existing archive for the day was merged in.
  pub remerged: bool,
  /// Copies of records dropped because they came from merging the same
  /// file twice.
  pub duplicates: u64,
  /// Torn tails dropped.
  pub torn: u64,
  pub archive: Archive,
}

/// The days with live files under `journal_root` that are closed at `now`
/// (unix ms): they ended at least `grace` ago. Oldest first.
pub fn closed_days(
  journal_root: &Path,
  now: i64,
  grace: Duration,
) -> io::Result<Vec<String>> {
  let Some(entries) = fs::read_dir(live_dir(journal_root)).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut days = Vec::new();
  for entry in entries {
    let entry = entry?;
    let day = entry.file_name().to_string_lossy().into_owned();
    if entry.file_type()?.is_dir()
      && valid_day(&day)
      && let Some(start) = day_start(&day)
      && now >= start + DAY_MS + grace.as_millis() as i64
    {
      days.push(day);
    }
  }
  days.sort();
  Ok(days)
}

/// One input's records, without their ends, in the newest version, and how
/// much of it was read.
struct Input {
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

/// Splits a journal's bytes into its records, and whether it has a torn
/// tail.
fn parse(path: &Path, bytes: &[u8]) -> io::Result<(Vec<Vec<u8>>, bool)> {
  let Some(header_end) = bytes.iter().position(|&b| b == b'\n') else {
    return Err(invalid(path, "no complete header"));
  };
  let header = Header::parse(&bytes[..header_end])
    .ok_or_else(|| invalid(path, "not a journal"))?;
  if header.variant != Variant::Dgg {
    return Err(invalid(path, "not dgg's journal"));
  }
  if header.version != v0::VERSION {
    return Err(invalid(path, "journal of an older version"));
  }
  let (records, tail) = header.framing().split(&bytes[header_end + 1..]);
  let records = records
    .into_iter()
    .filter(|record| !record.is_empty())
    .map(<[u8]>::to_vec)
    .collect();
  Ok((records, !tail.is_empty()))
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

/// Whether a backfill run still writes the live file at `path`: its
/// instance's, `<instance>.journal`, or `<instance>.<n>.journal` after a
/// change of format.
fn busy_file(journal_root: &Path, path: &Path) -> io::Result<bool> {
  let stem = path.file_stem().unwrap().to_string_lossy();
  let numbered = stem
    .rsplit_once('.')
    .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
  if writing(journal_root, &stem)? {
    return Ok(true);
  }
  match numbered {
    Some((instance, _)) => writing(journal_root, instance),
    None => Ok(false),
  }
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compacts `day`'s live files (and existing archive) under `journal_root`
/// into its archive, at zstd `level`. `None` if it has no live files that
/// aren't [busy](Day::busy).
pub fn compact_day(
  journal_root: &Path,
  day: &str,
  level: i32,
) -> io::Result<Option<Day>> {
  let mut inputs = Vec::new();
  let archive_path = archive_file(journal_root, day);
  let remerged = archive_path.exists();
  if remerged {
    let bytes = ::zstd::decode_all(File::open(&archive_path)?)?;
    let (records, torn) = parse(&archive_path, &bytes)?;
    inputs.push(Input {
      records,
      torn,
      live: None,
    });
  }
  let (mut live, mut busy) = (Vec::new(), 0);
  let Some(entries) =
    fs::read_dir(live_day_dir(journal_root, day)).missing_ok()?
  else {
    return Ok(None);
  };
  for entry in entries {
    let path = entry?.path();
    if !path
      .extension()
      .is_some_and(|extension| extension == EXTENSION)
    {
      continue;
    }
    if busy_file(journal_root, &path)? {
      busy += 1;
    } else {
      live.push(path);
    }
  }
  if live.is_empty() {
    return Ok(None);
  }
  live.sort();
  for path in live {
    let bytes = fs::read(&path)?;
    let (records, torn) = parse(&path, &bytes)?;
    inputs.push(Input {
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
  let header = Header::new(Variant::Dgg, v0::VERSION);
  let mut journal = header.text().into_bytes();
  for record in &records {
    journal.extend_from_slice(record);
    journal.extend_from_slice(header.framing().end());
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
  archive::record(journal_root, Variant::Dgg, &archive)?;

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
    remerged,
    duplicates: (total - records.len()) as u64,
    torn: inputs.iter().filter(|input| input.torn).count() as u64,
    archive,
  }))
}
