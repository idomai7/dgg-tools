//! Tests' own directories.

use {::common::temp_dir::TempDir, ::std::fs};

#[test]
fn made_empty_and_removed() {
  let dir = TempDir::new("common-temp-dir");
  let path = dir.path().to_path_buf();
  assert!(path.is_dir());
  assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
  let name = path.file_name().unwrap().to_str().unwrap();
  assert!(
    name.starts_with(&format!("common-temp-dir-{}-", ::std::process::id()))
  );
  dir.write("a/b/c.txt", "text");
  assert_eq!(fs::read_to_string(dir.join("a/b/c.txt")).unwrap(), "text");
  drop(dir);
  assert!(!path.exists());
}

#[test]
fn each_its_own() {
  let (a, b) = (
    TempDir::new("common-temp-dir"),
    TempDir::new("common-temp-dir"),
  );
  assert_ne!(a.path(), b.path());
  // It derefs to its path.
  assert_eq!(a.join("x"), a.path().join("x"));
}
