//! Reading, writing and counting days in unix milliseconds.

use ::common::time::{
  DAY_MS, civil_from_days, day_start, days_from_civil, now_ms, rfc3339,
  unix_ms, utc_day, utc_timestamp,
};

#[test]
fn now_is_now() {
  // After this was written, and before 2100.
  assert!((1_795_000_000_000..4_102_444_800_000).contains(&now_ms()));
}

#[test]
fn reads_rfc3339() {
  assert_eq!(
    unix_ms("2026-11-19T12:01:56+00:00"),
    Some(1_795_089_716_000)
  );
  assert_eq!(unix_ms("2026-11-19T12:07:30Z"), Some(1_795_090_050_000));
  assert_eq!(unix_ms("2026-11-19t12:07:30z"), Some(1_795_090_050_000));
  assert_eq!(unix_ms("1970-01-01T00:00:00Z"), Some(0));
  assert_eq!(unix_ms("1969-12-31T23:59:59+00:00"), Some(-1_000));
  assert_eq!(
    unix_ms("2024-02-29T23:59:59.999-05:30"),
    Some(1_709_270_999_999)
  );
  assert_eq!(unix_ms("2000-03-01T00:00:00+14:00"), Some(951_818_400_000));
  assert_eq!(unix_ms("2026-11-08T09:01:44.6Z"), Some(1_794_128_504_600));
  // Cut to milliseconds, not rounded.
  assert_eq!(unix_ms("1970-01-01T00:00:00.1Z"), Some(100));
  assert_eq!(unix_ms("1970-01-01T00:00:00.123999Z"), Some(123));
}

#[test]
fn refuses_what_isnt_rfc3339() {
  for s in [
    "",
    "x",
    "2026-11-19",
    "2026-11-19T12:01:56",
    "2026-11-08T09:01Z",
    "2026-11-08T09:01:44.611",
    "2026-11-19 12:01:56Z",
    "2026-11-19T12:01:56+0000",
    "2026-11-19T12:01:56+00:00 ",
    "2026-11-19T12:01:56.Z",
    "2026-1-19T12:01:56Z",
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

#[test]
fn writes_timestamps() {
  assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
  assert_eq!(rfc3339(1_794_168_297_005), "2026-11-08T20:04:57.005Z");
  assert_eq!(rfc3339(-1), "1969-12-31T23:59:59.999Z");
  assert_eq!(utc_timestamp(0), "1970-01-01 00:00:00.000Z");
  assert_eq!(utc_timestamp(1_794_128_504_611), "2026-11-08 09:01:44.611Z");
  for ms in [0, -1, 1_709_251_199_999, 1_794_168_297_005] {
    assert_eq!(unix_ms(&rfc3339(ms)), Some(ms));
  }
}

#[test]
fn days() {
  assert_eq!(utc_day(0), "1970-01-01");
  assert_eq!(utc_day(-1), "1969-12-31");
  assert_eq!(utc_day(1_709_164_800_000), "2024-02-29");
  assert_eq!(utc_day(1_709_251_199_999), "2024-02-29");
  assert_eq!(utc_day(1_794_128_504_611), "2026-11-08");
  assert_eq!(day_start("1970-01-01"), Some(0));
  assert_eq!(day_start("2024-02-29"), Some(1_709_164_800_000));
  for bad in ["", "2024-02-30", "2024-2-29", "2024-02-29T00:00:00Z"] {
    assert_eq!(day_start(bad), None, "{bad}");
  }
}

#[test]
fn the_calendar_both_ways() {
  assert_eq!(days_from_civil(1970, 1, 1), 0);
  assert_eq!(days_from_civil(1969, 12, 31), -1);
  assert_eq!(days_from_civil(2000, 3, 1), 11_017);
  // Every day from 1600 to 2400, there and back.
  let (from, to) = (days_from_civil(1600, 1, 1), days_from_civil(2400, 1, 1));
  let mut expected = (1600, 1, 1);
  for days in from..to {
    let date = civil_from_days(days);
    assert_eq!(date, expected, "{days}");
    assert_eq!(days_from_civil(date.0, date.1, date.2), days);
    expected = next(date);
  }
  assert_eq!(DAY_MS, 86_400 * 1_000);
}

/// The day after `date`.
fn next((year, month, day): (i64, i64, i64)) -> (i64, i64, i64) {
  let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
  let length = match month {
    2 if leap => 29,
    2 => 28,
    4 | 6 | 9 | 11 => 30,
    _ => 31,
  };
  match (day < length, month < 12) {
    (true, _) => (year, month, day + 1),
    (false, true) => (year, month + 1, 1),
    (false, false) => (year + 1, 1, 1),
  }
}
