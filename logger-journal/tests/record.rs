use ::logger_journal::record::Version;

fn version(major: u32, minor: u32) -> Version {
  Version { major, minor }
}

#[test]
fn readable() {
  // Within major 0, only the exact minor: 0.3. Every host's journal was
  // migrated from 0.1 and 0.2.
  assert!(version(0, 3).readable());
  for minor in [0, 1, 2] {
    assert!(!version(0, minor).readable());
  }
  assert!(!version(0, 4).readable());
  assert!(!version(1, 0).readable());
}
