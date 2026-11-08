use ::journal::record::{Version, v1};

fn version(major: u32, minor: u32) -> Version {
  Version { major, minor }
}

#[test]
fn header() {
  assert_eq!(v1::VERSION.header(), "journal 1.1");
  let parse = |s: &str| Version::parse_header(s.as_bytes());
  assert_eq!(parse("journal 1.7"), Some(version(1, 7)));
  assert_eq!(parse("journal 12.0"), Some(version(12, 0)));
  for bad in [
    "journal 1",
    "journal 1.",
    "journal .1",
    "journal +1.0",
    "journal 1.0 ",
    "journal  1.0",
    "x 1.0",
  ] {
    assert_eq!(parse(bad), None, "{bad:?}");
  }
}

#[test]
fn readable() {
  assert!(version(1, 0).readable());
  assert!(version(1, 99).readable());
  assert!(!version(0, 1).readable());
  assert!(!version(2, 0).readable());
}
