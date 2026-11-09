//! Compact: merge each closed day's live files into one zstd archive.
//!
//! A day is closed once it ended (UTC) more than a grace period ago, so no
//! capture is still writing to it. Its live files, and its existing archive
//! if it was compacted before (a late `sync` can bring more files), are
//! merged line for line, byte for byte: sorted by receive time, keeping the
//! order of lines with equal times. The only lines dropped are copies made
//! by merging the same file twice (each distinct line is kept as often as
//! any one input has it), and torn tails left by crashes, which can't ever
//! be read.
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
  ::journal::{
    archive::{self, Archive, valid_day},
    layout::{EXTENSION, archive_file, live_day_dir, live_dir},
    record::{Version, v0},
    writer::writing,
  },
  ::sha2::{Digest, Sha256},
  ::std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
  },
};

const DAY_MS: i64 = 86_400_000;

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
  /// Copies of lines dropped because they came from merging the same file
  /// twice.
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
  let entries = match fs::read_dir(live_dir(journal_root)) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(e),
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

/// Unix ms at the start of `day` (`YYYY-MM-DD`), UTC.
fn day_start(day: &str) -> Option<i64> {
  let year: i64 = day[..4].parse().ok()?;
  let month: i64 = day[5..7].parse().ok()?;
  let day: i64 = day[8..].parse().ok()?;
  // Howard Hinnant's days_from_civil.
  let year = if month <= 2 { year - 1 } else { year };
  let era = year.div_euclid(400);
  let yoe = year - era * 400;
  let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
  let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  Some((era * 146_097 + doe - 719_468) * DAY_MS)
}

/// One input's lines, and how much of it was read.
struct Input {
  version: Version,
  lines: Vec<Vec<u8>>,
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

/// Splits a journal's bytes into its version and its lines.
fn parse(
  path: &Path,
  bytes: &[u8],
) -> io::Result<(Version, Vec<Vec<u8>>, bool)> {
  let Some(header_end) = bytes.iter().position(|&b| b == b'\n') else {
    return Err(invalid(path, "no complete header"));
  };
  let version = Version::parse_header(&bytes[..header_end])
    .ok_or_else(|| invalid(path, "not a journal"))?;
  if !version.readable() {
    return Err(invalid(path, "journal of an unreadable version"));
  }
  let body = &bytes[header_end + 1..];
  let mut lines: Vec<Vec<u8>> =
    body.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
  // What follows the last newline: empty, or a torn line.
  let tail = lines.pop().unwrap_or_default();
  lines.retain(|line| !line.is_empty());
  Ok((version, lines, !tail.is_empty()))
}

/// A line's receive time, the first field; unreadable ones sort last.
fn receive_time(line: &[u8]) -> i64 {
  let end = line.iter().position(|&b| b == b' ').unwrap_or(line.len());
  ::std::str::from_utf8(&line[..end])
    .ok()
    .and_then(|field| field.parse().ok())
    .unwrap_or(i64::MAX)
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
    let (version, lines, torn) = parse(&archive_path, &bytes)?;
    inputs.push(Input {
      version,
      lines,
      torn,
      live: None,
    });
  }
  let (mut live, mut busy) = (Vec::new(), 0);
  let entries = match fs::read_dir(live_day_dir(journal_root, day)) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(e),
  };
  for entry in entries {
    let path = entry?.path();
    if !path
      .extension()
      .is_some_and(|extension| extension == EXTENSION)
    {
      continue;
    }
    let instance = path.file_stem().unwrap().to_string_lossy();
    if writing(journal_root, &instance)? {
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
    let (version, lines, torn) = parse(&path, &bytes)?;
    inputs.push(Input {
      version,
      lines,
      torn,
      live: Some((path, bytes.len() as u64)),
    });
  }

  // Merge: by receive time, stably. A line repeated within one input is
  // real (dgg can send the same frame twice in a millisecond), so each
  // distinct line is kept as many times as the input with the most copies
  // has it; extra copies only come from merging the same file twice.
  let mut keep: HashMap<&[u8], usize> = HashMap::new();
  for input in &inputs {
    let mut counts: HashMap<&[u8], usize> = HashMap::new();
    for line in &input.lines {
      *counts.entry(line).or_default() += 1;
    }
    for (line, count) in counts {
      let most = keep.entry(line).or_default();
      *most = (*most).max(count);
    }
  }
  let minor = inputs
    .iter()
    .map(|input| input.version.minor)
    .chain([v0::VERSION.minor])
    .max()
    .unwrap();
  let mut lines: Vec<&[u8]> = inputs
    .iter()
    .flat_map(|input| input.lines.iter().map(Vec::as_slice))
    .collect();
  lines.sort_by_key(|line| receive_time(line));
  let total = lines.len();
  lines.retain(|line| {
    let left = keep.get_mut(line).unwrap();
    let kept = *left > 0;
    *left = left.saturating_sub(1);
    kept
  });
  let version = Version {
    major: v0::VERSION.major,
    minor,
  };
  let mut journal = format!("{}\n", version.header()).into_bytes();
  for line in &lines {
    journal.extend_from_slice(line);
    journal.push(b'\n');
  }

  // Write, verify, then put in place.
  let compressed = ::zstd::encode_all(journal.as_slice(), level)?;
  fs::create_dir_all(archive_path.parent().unwrap())?;
  let mut temporary = archive_path.clone().into_os_string();
  temporary.push(".tmp");
  let temporary = PathBuf::from(temporary);
  let mut file = File::create(&temporary)?;
  file.write_all(&compressed)?;
  file.sync_all()?;
  drop(file);
  let written = fs::read(&temporary)?;
  if written != compressed || ::zstd::decode_all(written.as_slice())? != journal
  {
    let _ = fs::remove_file(&temporary);
    return Err(invalid(&temporary, "archive didn't read back as written"));
  }
  fs::rename(&temporary, &archive_path)?;
  let archive = Archive {
    day: day.into(),
    compressed: compressed.len() as u64,
    bytes: journal.len() as u64,
    lines: lines.len() as u64,
    sha256: hex(&Sha256::digest(&compressed)),
  };
  archive::record(journal_root, &archive)?;

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
    duplicates: (total - lines.len()) as u64,
    torn: inputs.iter().filter(|input| input.torn).count() as u64,
    archive,
  }))
}
