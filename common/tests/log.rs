use ::common::log::Level;

#[test]
fn levels_are_padded_and_split_back() {
  assert_eq!(format!("{} x", Level::Info), "INFO  x");
  assert_eq!(format!("{} x", Level::Error), "ERROR x");
  for level in [Level::Info, Level::Warn, Level::Error] {
    let line = format!("{level} a  b");
    assert_eq!(Level::split(&line), Some((level, "a  b")));
  }
  assert_eq!(Level::split("INFORMATION x"), None);
  assert_eq!(Level::split("a-1: capturing"), None);
  assert_eq!(Level::split("WARN"), None);
}
