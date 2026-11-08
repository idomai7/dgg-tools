//! Appends records to live files, always in the [`current`] format.

use {
  crate::{
    current::{self, Body},
    layout::{live_file, utc_day},
    version::CURRENT,
  },
  ::std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    time::{Duration, Instant},
  },
};

/// How often [`Writer::sync_if_due`] fsyncs.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// Appends one instance's records to its live files, rolling over at UTC
/// midnight (by each record's `recv`).
///
/// Every line is written with a single `write` call, so a crash leaves at
/// most one torn line at the tail. Call [`Writer::sync_if_due`] on a timer to
/// bound how much a power loss can take.
pub struct Writer {
  root: PathBuf,
  inst: String,
  open: Option<(String, File)>,
  dirty: bool,
  last_sync: Instant,
}

impl Writer {
  pub fn new(root: impl Into<PathBuf>, inst: impl Into<String>) -> Self {
    let inst = inst.into();
    assert!(
      current::valid_inst(&inst),
      "instance id must be [A-Za-z0-9._-]+: {inst:?}"
    );
    Self {
      root: root.into(),
      inst,
      open: None,
      dirty: false,
      last_sync: Instant::now(),
    }
  }

  pub fn inst(&self) -> &str {
    &self.inst
  }

  pub fn write(&mut self, recv: i64, conn: u64, body: Body) -> io::Result<()> {
    let day = utc_day(recv);
    if self.open.as_ref().is_none_or(|(d, _)| *d != day) {
      self.sync()?;
      self.open = Some((day.clone(), self.open_day(&day)?));
    }
    let rec = current::Record {
      recv,
      inst: self.inst.clone(),
      conn,
      body,
    };
    let mut line = Vec::new();
    rec.encode(&mut line);
    let (_, file) = self.open.as_mut().unwrap();
    file.write_all(&line)?;
    self.dirty = true;
    Ok(())
  }

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

  pub fn sync_if_due(&mut self) -> io::Result<()> {
    if self.last_sync.elapsed() >= SYNC_INTERVAL {
      self.sync()?;
    }
    Ok(())
  }

  fn open_day(&self, day: &str) -> io::Result<File> {
    let path = live_file(&self.root, day, &self.inst);
    fs::create_dir_all(path.parent().unwrap())?;
    let mut file = OpenOptions::new()
      .create(true)
      .read(true)
      .append(true)
      .open(path)?;
    if file.metadata()?.len() == 0 {
      file.write_all(format!("{}\n", CURRENT.header()).as_bytes())?;
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
  }
}
