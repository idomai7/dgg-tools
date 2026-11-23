use {
  ::common::temp_dir::TempDir,
  ::common::time::utc_timestamp,
  ::deploy::log::{Log, rotated},
  ::std::{
    fs,
    path::{Path, PathBuf},
  },
};

#[test]
fn rotated_names_keep_the_extension() {
  let path = PathBuf::from("logs").join("capture.log");
  assert_eq!(
    rotated(&path, 2),
    PathBuf::from("logs").join("capture.2.log")
  );
}

#[test]
fn rotates_by_size_keeping_some() {
  let dir = TempDir::new("deploy-log-rotate");
  let path = dir.path().join("capture.log");
  // Each line is a 25-byte timestamp, a space, 4 bytes and a newline.
  let mut log = Log::open(&path, 70, 2).unwrap();
  for n in 0..7 {
    log.line_at(0, &format!("line{n}")).unwrap();
  }
  drop(log);
  let read = |path: &Path| fs::read_to_string(path).unwrap();
  let stamp = utc_timestamp(0);
  let lines = |ns: &[u32]| -> String {
    ns.iter().map(|n| format!("{stamp} line{n}\n")).collect()
  };
  // Two lines fit in 70 bytes; the oldest pair is gone.
  assert_eq!(read(&path), lines(&[6]));
  assert_eq!(read(&rotated(&path, 1)), lines(&[4, 5]));
  assert_eq!(read(&rotated(&path, 2)), lines(&[2, 3]));
  assert!(!rotated(&path, 3).exists());
}

#[test]
fn appends_to_an_existing_log() {
  let dir = TempDir::new("deploy-log-append");
  let path = dir.path().join("index.log");
  Log::open(&path, 1 << 20, 1)
    .unwrap()
    .line_at(0, "first")
    .unwrap();
  Log::open(&path, 1 << 20, 1)
    .unwrap()
    .line_at(0, "second")
    .unwrap();
  let stamp = utc_timestamp(0);
  assert_eq!(
    fs::read_to_string(&path).unwrap(),
    format!("{stamp} first\n{stamp} second\n")
  );
}
