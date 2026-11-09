//! Log files with the time on every line, rotated by size.
//!
//! `capture.log` grows to `max_size`, then becomes `capture.1.log` (the
//! previous `.1` becoming `.2`, and so on), keeping `keep` old files.

pub use ::logger::capture::{now_ms, timestamp};

use ::std::{
  fs::{self, File, OpenOptions},
  io::{self, Write},
  path::{Path, PathBuf},
};

/// `<stem>.<n>.<extension>` beside `path`: its `n`th rotated copy.
pub fn rotated(path: &Path, n: u32) -> PathBuf {
  let stem = path.file_stem().unwrap_or_default().to_string_lossy();
  let name = match path.extension() {
    Some(extension) => format!("{stem}.{n}.{}", extension.to_string_lossy()),
    None => format!("{stem}.{n}"),
  };
  path.with_file_name(name)
}

pub struct Log {
  path: PathBuf,
  file: File,
  size: u64,
  max_size: u64,
  keep: u32,
}

impl Log {
  /// Opens `path` for appending, creating it and its directory if needed.
  pub fn open(
    path: impl Into<PathBuf>,
    max_size: u64,
    keep: u32,
  ) -> io::Result<Self> {
    let path = path.into();
    if let Some(directory) = path.parent() {
      fs::create_dir_all(directory)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let size = file.metadata()?.len();
    Ok(Self {
      path,
      file,
      size,
      max_size,
      keep,
    })
  }

  /// Appends `line` with the time in front.
  pub fn line(&mut self, line: &str) -> io::Result<()> {
    self.line_at(now_ms(), line)
  }

  /// Appends `line` with unix ms `ms` in front, rotating first if it would
  /// take the file past its maximum size.
  pub fn line_at(&mut self, ms: i64, line: &str) -> io::Result<()> {
    let text = format!("{} {line}\n", timestamp(ms));
    if self.size > 0 && self.size + text.len() as u64 > self.max_size {
      self.rotate()?;
    }
    self.file.write_all(text.as_bytes())?;
    self.size += text.len() as u64;
    Ok(())
  }

  fn rotate(&mut self) -> io::Result<()> {
    let ignore_missing = |result: io::Result<()>| match result {
      Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
      _ => Ok(()),
    };
    if self.keep == 0 {
      ignore_missing(fs::remove_file(&self.path))?;
    } else {
      ignore_missing(fs::remove_file(rotated(&self.path, self.keep)))?;
      for n in (1..self.keep).rev() {
        let from = rotated(&self.path, n);
        ignore_missing(fs::rename(from, rotated(&self.path, n + 1)))?;
      }
      fs::rename(&self.path, rotated(&self.path, 1))?;
    }
    self.file = OpenOptions::new()
      .create(true)
      .append(true)
      .open(&self.path)?;
    self.size = 0;
    Ok(())
  }
}
