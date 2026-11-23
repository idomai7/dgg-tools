//! Replacing a file whole. It's written to a temporary file beside it,
//! `<name>.tmp`, flushed to disk, and renamed over it, so a reader sees the
//! old file or the new one, never half of one, even after a crash.
//!
//! On Windows a rename fails while another process has the file open, so
//! it's retried for a while before giving up. When anything fails, the
//! temporary file is removed and the file is left as it was.

use ::std::{
  fs::{self, File, OpenOptions},
  io::{self, Write},
  path::{Path, PathBuf},
  thread,
  time::Duration,
};

/// Tries at renaming over the file, this far apart.
const RENAME_TRIES: u32 = 10;
const RENAME_WAIT: Duration = Duration::from_millis(100);

/// Replaces `path` with `contents`, creating its directory if need be.
pub fn replace(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
  replace_with(path, |file| file.write_all(contents.as_ref()))
}

/// Replaces `path` with what `fill` writes, creating its directory if need
/// be. The file is open for reading too, so `fill` may read back what it
/// wrote, to check it; its error leaves `path` as it was.
pub fn replace_with(
  path: &Path,
  fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
  if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
    fs::create_dir_all(dir)?;
  }
  let temporary = temporary(path);
  let mut file = OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(true)
    .open(&temporary)?;
  let written = fill(&mut file).and_then(|()| file.sync_all());
  drop(file);
  written
    .and_then(|()| rename(&temporary, path))
    .inspect_err(|_| {
      let _ = fs::remove_file(&temporary);
    })
}

/// Where `path` is written before it's renamed into place: `<path>.tmp`.
pub fn temporary(path: &Path) -> PathBuf {
  let mut temporary = path.as_os_str().to_owned();
  temporary.push(".tmp");
  temporary.into()
}

/// Renames `from` over `to`, trying a few times: Windows refuses while
/// another process has `to` open.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
  let mut tries = 1;
  loop {
    match fs::rename(from, to) {
      Err(_) if tries < RENAME_TRIES => {
        tries += 1;
        thread::sleep(RENAME_WAIT);
      }
      done => return done,
    }
  }
}
