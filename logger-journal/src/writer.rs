//! Appends records to live or backfill files, always in the [`v0`] format.

use {
  crate::{
    layout::{backfill_file, backfill_root, live_file, lock_file, utc_day},
    record::v0::{self, Body},
  },
  ::std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
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
/// Every line is written with a single `write` call, so a crash leaves at
/// most one torn line at the tail. Call [`Writer::sync_if_due`] on a timer to
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

  /// The instance id written on every line.
  pub fn instance(&self) -> &str {
    &self.instance
  }

  /// Appends one line to the live file for `receive_time`'s UTC day, opening
  /// (and on a day change, syncing and switching) files as needed. The line
  /// reaches the OS before this returns, but isn't fsynced; see
  /// [`Writer::sync`].
  pub fn write(
    &mut self,
    receive_time: i64,
    connection: u64,
    body: Body,
  ) -> io::Result<()> {
    self.write_on(receive_time, receive_time, connection, body)
  }

  /// Like [`Writer::write`], but files the line under `file_time`'s UTC day
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
    let mut line = Vec::new();
    record.encode(&mut line);
    let (_, file) = self.open.as_mut().unwrap();
    file.write_all(&line)?;
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

  /// Opens (creating if needed) this instance's file for `day`. A new file
  /// gets the v0 header; an existing one with a torn tail gets a newline.
  fn open_day(&self, day: &str) -> io::Result<File> {
    let path = if self.backfill {
      backfill_file(&self.root, day, &self.instance)
    } else {
      live_file(&self.root, day, &self.instance)
    };
    fs::create_dir_all(path.parent().unwrap())?;
    let mut file = OpenOptions::new()
      .create(true)
      .read(true)
      .append(true)
      .open(path)?;
    if file.metadata()?.len() == 0 {
      file.write_all(format!("{}\n", v0::VERSION.header()).as_bytes())?;
    } else {
      // Isolate a torn line left by a crash, so the next record starts clean.
      let mut last = [0];
      file.seek(SeekFrom::End(-1))?;
      file.read_exact(&mut last)?;
      if last[0] != b'\n' {
        file.write_all(b"\n")?;
      }
    }
    Ok(file)
  }
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
  let file = match File::open(lock_file(root, instance)) {
    Ok(file) => file,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
    Err(e) => return Err(e),
  };
  match file.try_lock_shared() {
    Ok(()) => Ok(false),
    Err(TryLockError::WouldBlock) => Ok(true),
    Err(TryLockError::Error(e)) => Err(e),
  }
}
