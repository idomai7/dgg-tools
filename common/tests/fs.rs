//! Files and directories, in a directory of the test's own.

use {
  ::common::fs::{MissingOk, files, move_file, remove_empty_dirs, same},
  ::common::temp_dir::TempDir,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
  },
};

#[test]
fn missing_is_none() {
  let dir = TempDir::new("common-fs");
  let missing = dir.path().join("missing");
  assert_eq!(fs::read_to_string(&missing).missing_ok().unwrap(), None);
  assert!(fs::read_dir(&missing).missing_ok().unwrap().is_none());
  assert_eq!(fs::remove_file(&missing).missing_ok().unwrap(), None);
  dir.write("there", "text");
  let there = fs::read_to_string(dir.path().join("there")).missing_ok();
  assert_eq!(there.unwrap().as_deref(), Some("text"));
  // Other errors stay errors.
  let other: io::Result<()> = Err(io::Error::other("no"));
  assert!(other.missing_ok().is_err());
}

#[test]
fn lists_every_file_under_a_directory() {
  let dir = TempDir::new("common-fs");
  dir.write("b/c/d.txt", "");
  dir.write("a.txt", "");
  dir.write("b/e.txt", "");
  fs::create_dir_all(dir.path().join("empty")).unwrap();
  let found = files(dir.path()).unwrap();
  let expected: Vec<PathBuf> = ["a.txt", "b/c/d.txt", "b/e.txt"]
    .iter()
    .map(|path| path.split('/').collect())
    .collect();
  assert_eq!(found, expected);
  assert!(files(&dir.path().join("missing")).unwrap().is_empty());
}

#[test]
fn moves_files_and_tidies_up() {
  let dir = TempDir::new("common-fs");
  dir.write("from/deep/file", "moved");
  let to = dir.path().join("to").join("new").join("file");
  move_file(&dir.path().join("from/deep/file"), &to).unwrap();
  assert_eq!(fs::read_to_string(&to).unwrap(), "moved");
  assert!(!dir.path().join("from/deep/file").exists());

  // Only the empty directories go.
  dir.write("from/kept/file", "");
  remove_empty_dirs(&dir.path().join("from"));
  assert!(!dir.path().join("from/deep").exists());
  assert!(dir.path().join("from/kept/file").exists());
  fs::remove_file(dir.path().join("from/kept/file")).unwrap();
  remove_empty_dirs(&dir.path().join("from"));
  assert!(!dir.path().join("from").exists());
}

#[test]
fn same_places() {
  let dir = TempDir::new("common-fs");
  fs::create_dir_all(dir.path().join("a")).unwrap();
  assert!(same(&dir.path().join("a"), &dir.path().join("a").join(".")));
  assert!(!same(&dir.path().join("a"), dir.path()));
  assert!(same(Path::new("missing"), Path::new("missing")));
  assert!(!same(Path::new("missing"), Path::new("other")));
}
