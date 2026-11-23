//! Replacing files whole, in a directory of the test's own.

use {
  ::common::atomic_file::{replace, replace_with, temporary},
  ::common::temp_dir::TempDir,
  ::std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
  },
};

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
  let dir = TempDir::new("common-atomic-file");
  // Its directory doesn't exist yet.
  let path = dir.path().join("a").join("b.json");
  replace(&path, "first").unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), "first");
  replace(&path, b"second").unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), "second");
  assert_eq!(names(&dir.path().join("a")), ["b.json"]);
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
  let dir = TempDir::new("common-atomic-file");
  let path = dir.path().join("checked");
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
  let dir = TempDir::new("common-atomic-file");
  let path = dir.path().join("kept");
  replace(&path, "old").unwrap();
  let failed = replace_with(&path, |file| {
    file.write_all(b"half")?;
    Err(io::Error::other("didn't check out"))
  });
  assert_eq!(failed.unwrap_err().to_string(), "didn't check out");
  assert_eq!(fs::read_to_string(&path).unwrap(), "old");
  assert_eq!(names(dir.path()), ["kept"]);

  // Renaming over a directory fails, after its tries.
  let blocked = dir.path().join("blocked");
  fs::create_dir_all(blocked.join("inside")).unwrap();
  assert!(replace(&blocked, "new").is_err());
  assert_eq!(names(dir.path()), ["blocked", "kept"]);
}
