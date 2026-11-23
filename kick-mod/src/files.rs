//! Writing kick-mod's files.

use ::std::{fs, io, path::Path, thread, time::Duration};

/// Tries at renaming over a file: on Windows it fails while another process
/// has the file open.
const RENAME_TRIES: u32 = 10;
const RENAME_WAIT: Duration = Duration::from_millis(100);

/// Replaces `path` with `text`, creating its directory if need be. It's
/// written to a temporary file beside it, which is renamed over it, so a
/// reader sees the old file or the new one, never half of one.
pub fn replace(path: &Path, text: &str) -> io::Result<()> {
  if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
    fs::create_dir_all(dir)?;
  }
  let mut temporary = path.as_os_str().to_owned();
  temporary.push(".tmp");
  fs::write(&temporary, text)?;
  let mut tries = 1;
  loop {
    match fs::rename(&temporary, path) {
      Ok(()) => return Ok(()),
      Err(_) if tries < RENAME_TRIES => {
        tries += 1;
        thread::sleep(RENAME_WAIT);
      }
      Err(e) => {
        let _ = fs::remove_file(&temporary);
        return Err(e);
      }
    }
  }
}
