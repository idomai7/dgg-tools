//! Replacing files whole, in a directory of the test's own.

use {
  ::common::atomic_file::{replace, replace_with, temporary},
  ::std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
  },
};

/// A directory of its own, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
  fn new() -> Self {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir().join(format!(
      "common-atomic-file-test-{}-{n}",
      ::std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    Self(dir)
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

/// The names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
  let mut names: Vec<_> = fs::read_dir(dir)
    .unwrap()
    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
    .collect();
  names.sort();
  names
}

#[test]
fn writes_and_replaces() {
  let dir = TempDir::new();
  // Its directory doesn't exist yet.
  let path = dir.0.join("a").join("b.json");
  replace(&path, "first").unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), "first");
  replace(&path, b"second").unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), "second");
  assert_eq!(names(&dir.0.join("a")), ["b.json"]);
}

#[test]
fn the_temporary_file_is_beside_it() {
  assert_eq!(temporary(Path::new("x.json")), Path::new("x.json.tmp"));
  assert_eq!(
    temporary(Path::new("dir/x.json")),
    Path::new("dir").join("x.json.tmp")
  );
}

#[test]
fn fill_can_check_what_it_wrote() {
  let dir = TempDir::new();
  let path = dir.0.join("checked");
  replace_with(&path, |file| {
    file.write_all(b"written")?;
    file.seek(SeekFrom::Start(0))?;
    let mut back = String::new();
    file.read_to_string(&mut back)?;
    assert_eq!(back, "written");
    Ok(())
  })
  .unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), "written");
}

#[test]
fn a_failure_leaves_the_file_as_it_was() {
  let dir = TempDir::new();
  let path = dir.0.join("kept");
  replace(&path, "old").unwrap();
  let failed = replace_with(&path, |file| {
    file.write_all(b"half")?;
    Err(io::Error::other("didn't check out"))
  });
  assert_eq!(failed.unwrap_err().to_string(), "didn't check out");
  assert_eq!(fs::read_to_string(&path).unwrap(), "old");
  assert_eq!(names(&dir.0), ["kept"]);

  // Renaming over a directory fails, after its tries.
  let blocked = dir.0.join("blocked");
  fs::create_dir_all(blocked.join("inside")).unwrap();
  assert!(replace(&blocked, "new").is_err());
  assert_eq!(names(&dir.0), ["blocked", "kept"]);
}
