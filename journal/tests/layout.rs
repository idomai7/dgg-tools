use ::journal::layout::utc_day;

#[test]
fn utc_days() {
  assert_eq!(utc_day(0), "1970-01-01");
  assert_eq!(utc_day(-1), "1969-12-31");
  assert_eq!(utc_day(1_709_164_800_000), "2024-02-29");
  assert_eq!(utc_day(1_709_251_199_999), "2024-02-29");
  assert_eq!(utc_day(1_794_128_504_611), "2026-11-08");
}
