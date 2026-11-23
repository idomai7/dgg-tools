//! Appends records to live or backfill files, always in the newest
//! [`v0`] format; [`kick`] writes Kick's.
//!
//! A writer appends to a day's file only if its header is the one the writer
//! would write. Otherwise, as for a file of an older version, it starts the
//! day's next file, `<instance>.<n>.journal` (see [`numbered_live_file`]),
//! so new records never land in a file of another format.

pub mod kick;

use {
  crate::{
    layout::{backfill_root, lock_file, numbered_live_file},
    record::{
      END, Header, Variant,
      v0::{self, Body},
    },
  },
  ::common::{fs::MissingOk, time::utc_day},
  ::std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
  },
};

/// How often [`Writer::sync_if_due`] fsyncs.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// Appends one instance's records to its live files, rolling over at UTC
/// midnight (by each record's `receive_time`), or to its
/// [backfill](Writer::backfill) files.
///
/// Every record is written with a single `write` call, so a crash leaves at
/// most one torn record at the tail. Call [`Writer::sync_if_due`] on a timer to
/// bound how much a power loss can take.
pub struct Writer {
  root: PathBuf,
  instance: String,
  backfill: bool,
  /// For backfill: the held [`lock_file`], and its path.
  lock: Option<(PathBuf, File)>,
  open: Option<(String, File)>,
  dirty: bool,
  last_sync: Instant,
}

impl Writer {
  /// A writer for `instance` under journal `root`. Nothing is opened
  /// until the first [`Writer::write`].
  ///
  /// # Panics
  ///
  /// If `instance` isn't [`v0::valid_instance`].
  pub fn new(root: impl Into<PathBuf>, instance: impl Into<String>) -> Self {
    let instance = instance.into();
    assert!(
      v0::valid_instance(&instance),
      "instance id must be [A-Za-z0-9._-]+: {instance:?}"
    );
    Self {
      root: root.into(),
      instance,
      backfill: false,
      lock: None,
      open: None,
      dirty: false,
      last_sync: Instant::now(),
    }
  }

  /// A writer for `instance`'s backfill files under journal `root`, filed
  /// with [`Writer::write_on`] by the day of what they hold.
  ///
  /// Those days are usually closed already, so it holds `instance`'s
  /// [`lock_file`] until dropped: compaction leaves the files of a held
  /// instance alone (see [`writing`]). Fails if the lock is already held.
  ///
  /// # Panics
  ///
  /// If `instance` isn't [`v0::valid_instance`].
  pub fn backfill(
    root: impl Into<PathBuf>,
    instance: impl Into<String>,
  ) -> io::Result<Self> {
    let mut writer = Self::new(root, instance);
    writer.backfill = true;
    let path = lock_file(&backfill_root(&writer.root), &writer.instance);
    fs::create_dir_all(path.parent().unwrap())?;
    let file = File::create(&path)?;
    file.try_lock()?;
    writer.lock = Some((path, file));
    Ok(writer)
  }

  /// The instance id written on every record.
  pub fn instance(&self) -> &str {
    &self.instance
  }

  /// Appends one record to the live file for `receive_time`'s UTC day,
  /// opening (and on a day change, syncing and switching) files as needed.
  /// The record reaches the OS before this returns, but isn't fsynced; see
  /// [`Writer::sync`].
  pub fn write(
    &mut self,
    receive_time: i64,
    connection: u64,
    body: Body,
  ) -> io::Result<()> {
    self.write_on(receive_time, receive_time, connection, body)
  }

  /// Like [`Writer::write`], but files the record under `file_time`'s UTC day
  /// instead: for backfill, the time of the message it holds.
  pub fn write_on(
    &mut self,
    file_time: i64,
    receive_time: i64,
    connection: u64,
    body: Body,
  ) -> io::Result<()> {
    let day = utc_day(file_time);
    if self
      .open
      .as_ref()
      .is_none_or(|(current, _)| *current != day)
    {
      self.sync()?;
      self.open = Some((day.clone(), self.open_day(&day)?));
    }
    let record = v0::Record {
      receive_time,
      instance: self.instance.clone(),
      connection,
      body,
    };
    let mut encoded = Vec::new();
    record.encode(&mut encoded);
    let (_, file) = self.open.as_mut().unwrap();
    file.write_all(&encoded)?;
    self.dirty = true;
    Ok(())
  }

  /// Fsyncs the open file if anything was written since the last sync.
  pub fn sync(&mut self) -> io::Result<()> {
    if let Some((_, file)) = &self.open
      && self.dirty
    {
      file.sync_data()?;
    }
    self.dirty = false;
    self.last_sync = Instant::now();
    Ok(())
  }

  /// [`Writer::sync`]s if [`SYNC_INTERVAL`] has passed since the last sync.
  pub fn sync_if_due(&mut self) -> io::Result<()> {
    if self.last_sync.elapsed() >= SYNC_INTERVAL {
      self.sync()?;
    }
    Ok(())
  }

  /// Opens (creating if needed) this instance's file for `day`.
  fn open_day(&self, day: &str) -> io::Result<File> {
    let root = if self.backfill {
      backfill_root(&self.root)
    } else {
      self.root.clone()
    };
    let header = Header::new(Variant::Dgg, v0::VERSION);
    open_day(&root, day, &self.instance, &header)
  }
}

/// Opens `instance`'s last file for `day` under journal `root` to append to,
/// if its header is `header`'s line; otherwise, or if it has none, starts
/// the next, creating it and its directory as needed.
fn open_day(
  root: &Path,
  day: &str,
  instance: &str,
  header: &Header,
) -> io::Result<File> {
  let path = |n| numbered_live_file(root, day, instance, n);
  let mut n = 1;
  while path(n + 1).exists() {
    n += 1;
  }
  let text = header.text();
  if let Some(file) = File::open(path(n)).missing_ok()?
    && first_line(&file)? != text.as_bytes()
  {
    n += 1;
  }
  open_append(&path(n), &text)
}

/// `file`'s first line, up to and with its LF.
fn first_line(file: &File) -> io::Result<Vec<u8>> {
  let mut line = Vec::new();
  BufReader::new(file).read_until(b'\n', &mut line)?;
  Ok(line)
}

/// Opens `path` to append to, creating it if needed. A new file gets
/// `header`, with its end, in one write. An existing one with a torn tail
/// after its header gets the end of a record, so the next record starts
/// clean.
fn open_append(path: &Path, header: &str) -> io::Result<File> {
  fs::create_dir_all(path.parent().unwrap())?;
  let mut file = OpenOptions::new()
    .create(true)
    .read(true)
    .append(true)
    .open(path)?;
  let len = file.metadata()?.len();
  if len == 0 {
    file.write_all(header.as_bytes())?;
    return Ok(file);
  }
  let end = END;
  let mut tail = vec![0; end.len().min(len as usize)];
  file.seek(SeekFrom::End(-(tail.len() as i64)))?;
  file.read_exact(&mut tail)?;
  // Just the header needs nothing.
  let header_only = len == header.len() as u64;
  if tail != end && !header_only {
    file.write_all(end)?;
  }
  Ok(file)
}

impl Drop for Writer {
  fn drop(&mut self) {
    let _ = self.sync();
    if let Some((path, file)) = self.lock.take() {
      drop(file);
      let _ = fs::remove_file(path);
    }
  }
}

/// Whether a [backfill](Writer::backfill) writer holds `instance`'s
/// [`lock_file`] under journal `root` (a backfill root), so may still write
/// that instance's files.
pub fn writing(root: &Path, instance: &str) -> io::Result<bool> {
  let Some(file) = File::open(lock_file(root, instance)).missing_ok()? else {
    return Ok(false);
  };
  match file.try_lock_shared() {
    Ok(()) => Ok(false),
    Err(TryLockError::WouldBlock) => Ok(true),
    Err(TryLockError::Error(e)) => Err(e),
  }
}
