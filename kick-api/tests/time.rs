use ::kick_api::realtime::time::unix_ms;

#[test]
fn kicks_forms() {
  assert_eq!(
    unix_ms("2026-11-19T12:01:56+00:00"),
    Some(1_795_089_716_000)
  );
  assert_eq!(unix_ms("2026-11-19T12:07:30Z"), Some(1_795_090_050_000));
  assert_eq!(unix_ms("2026-11-19t12:07:30z"), Some(1_795_090_050_000));
}

#[test]
fn offsets_fractions_and_edges() {
  assert_eq!(unix_ms("1970-01-01T00:00:00Z"), Some(0));
  assert_eq!(unix_ms("1969-12-31T23:59:59+00:00"), Some(-1_000));
  assert_eq!(
    unix_ms("2024-02-29T23:59:59.999-05:30"),
    Some(1_709_270_999_999)
  );
  assert_eq!(unix_ms("2000-03-01T00:00:00+14:00"), Some(951_818_400_000));
  // Cut to milliseconds, not rounded.
  assert_eq!(unix_ms("1970-01-01T00:00:00.1Z"), Some(100));
  assert_eq!(unix_ms("1970-01-01T00:00:00.123999Z"), Some(123));
}

#[test]
fn refuses_the_rest() {
  for s in [
    "",
    "2026-11-19",
    "2026-11-19T12:01:56",
    "2026-11-19 12:01:56Z",
    "2026-11-19T12:01:56+0000",
    "2026-11-19T12:01:56+00:00 ",
    "2026-11-19T12:01:56.Z",
    "2026-13-01T00:00:00Z",
    "2026-02-29T00:00:00Z",
    "2026-04-31T00:00:00Z",
    "2026-11-19T24:00:00Z",
    "2026-11-19T12:60:00Z",
    "2016-12-31T23:59:60Z",
    "2026-11-19T12:01:56+24:00",
    "+026-11-19T12:01:56Z",
    "1795089716",
  ] {
    assert_eq!(unix_ms(s), None, "{s}");
  }
}
