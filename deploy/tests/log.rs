use {
  ::deploy::log::{Log, rotated, timestamp},
  ::std::{
    fs,
    path::{Path, PathBuf},
  },
};

struct TempDir(PathBuf);

impl TempDir {
  fn new(name: &str) -> Self {
    let path = ::std::env::temp_dir()
      .join(format!("deploy-log-{name}-{}", ::std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    Self(path)
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

#[test]
fn timestamps_are_utc() {
  assert_eq!(timestamp(0), "1970-01-01 00:00:00.000Z");
  assert_eq!(timestamp(1_794_168_240_123), "2026-11-08 20:04:00.123Z");
}

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
  let dir = TempDir::new("rotate");
  let path = dir.0.join("capture.log");
  // Each line is a 25-byte timestamp, a space, 4 bytes and a newline.
  let mut log = Log::open(&path, 70, 2).unwrap();
  for n in 0..7 {
    log.line_at(0, &format!("line{n}")).unwrap();
  }
  drop(log);
  let read = |path: &Path| fs::read_to_string(path).unwrap();
  let stamp = timestamp(0);
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
  let dir = TempDir::new("append");
  let path = dir.0.join("index.log");
  Log::open(&path, 1 << 20, 1)
    .unwrap()
    .line_at(0, "first")
    .unwrap();
  Log::open(&path, 1 << 20, 1)
    .unwrap()
    .line_at(0, "second")
    .unwrap();
  let stamp = timestamp(0);
  assert_eq!(
    fs::read_to_string(&path).unwrap(),
    format!("{stamp} first\n{stamp} second\n")
  );
}
