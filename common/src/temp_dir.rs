//! A directory of a test's own, removed when it's dropped.

use ::std::{
  fs,
  ops::Deref,
  path::{Path, PathBuf},
  sync::atomic::{AtomicU32, Ordering},
};

/// An empty directory under the OS's temporary directory, named
/// `<name>-<process id>-<count>` so tests running at once never share one,
/// and removed, with everything in it, when this is dropped. It derefs to
/// its path.
#[derive(Debug)]
pub struct TempDir {
  path: PathBuf,
}

impl TempDir {
  /// A new, empty directory. Panics if it can't be made: it's for tests.
  pub fn new(name: &str) -> Self {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let path = ::std::env::temp_dir()
      .join(format!("{name}-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("a temporary directory");
    Self { path }
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  /// Writes `contents` to `relative`, creating its directories. Panics if
  /// it can't.
  pub fn write(&self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = self.path.join(relative);
    if let Some(dir) = path.parent() {
      fs::create_dir_all(dir).expect("a directory");
    }
    fs::write(&path, contents).expect("a file");
  }
}

impl Deref for TempDir {
  type Target = Path;

  fn deref(&self) -> &Path {
    &self.path
  }
}

impl AsRef<Path> for TempDir {
  fn as_ref(&self) -> &Path {
    &self.path
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.path);
  }
}
