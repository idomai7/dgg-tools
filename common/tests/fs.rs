//! Files and directories, in a directory of the test's own.

use {
  ::common::fs::{MissingOk, files, move_file, remove_empty_dirs, same},
  ::std::{
    fs, io,
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
    let dir = ::std::env::temp_dir()
      .join(format!("common-fs-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    Self(dir)
  }

  fn write(&self, relative: &str, text: &str) {
    let path = self.0.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

#[test]
fn missing_is_none() {
  let dir = TempDir::new();
  let missing = dir.0.join("missing");
  assert_eq!(fs::read_to_string(&missing).missing_ok().unwrap(), None);
  assert!(fs::read_dir(&missing).missing_ok().unwrap().is_none());
  assert_eq!(fs::remove_file(&missing).missing_ok().unwrap(), None);
  dir.write("there", "text");
  let there = fs::read_to_string(dir.0.join("there")).missing_ok();
  assert_eq!(there.unwrap().as_deref(), Some("text"));
  // Other errors stay errors.
  let other: io::Result<()> = Err(io::Error::other("no"));
  assert!(other.missing_ok().is_err());
}

#[test]
fn lists_every_file_under_a_directory() {
  let dir = TempDir::new();
  dir.write("b/c/d.txt", "");
  dir.write("a.txt", "");
  dir.write("b/e.txt", "");
  fs::create_dir_all(dir.0.join("empty")).unwrap();
  let found = files(&dir.0).unwrap();
  let expected: Vec<PathBuf> = ["a.txt", "b/c/d.txt", "b/e.txt"]
    .iter()
    .map(|path| path.split('/').collect())
    .collect();
  assert_eq!(found, expected);
  assert!(files(&dir.0.join("missing")).unwrap().is_empty());
}

#[test]
fn moves_files_and_tidies_up() {
  let dir = TempDir::new();
  dir.write("from/deep/file", "moved");
  let to = dir.0.join("to").join("new").join("file");
  move_file(&dir.0.join("from/deep/file"), &to).unwrap();
  assert_eq!(fs::read_to_string(&to).unwrap(), "moved");
  assert!(!dir.0.join("from/deep/file").exists());

  // Only the empty directories go.
  dir.write("from/kept/file", "");
  remove_empty_dirs(&dir.0.join("from"));
  assert!(!dir.0.join("from/deep").exists());
  assert!(dir.0.join("from/kept/file").exists());
  fs::remove_file(dir.0.join("from/kept/file")).unwrap();
  remove_empty_dirs(&dir.0.join("from"));
  assert!(!dir.0.join("from").exists());
}

#[test]
fn same_places() {
  let dir = TempDir::new();
  fs::create_dir_all(dir.0.join("a")).unwrap();
  assert!(same(&dir.0.join("a"), &dir.0.join("a").join(".")));
  assert!(!same(&dir.0.join("a"), &dir.0));
  assert!(same(Path::new("missing"), Path::new("missing")));
  assert!(!same(Path::new("missing"), Path::new("other")));
}
