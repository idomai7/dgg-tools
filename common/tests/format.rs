//! Short forms for people to read.

use ::common::format::{count, duration, size};

#[test]
fn durations() {
  assert_eq!(duration(0), "0ms");
  assert_eq!(duration(-5), "0ms");
  assert_eq!(duration(850), "850ms");
  assert_eq!(duration(12_345), "12.3s");
  assert_eq!(duration(42_000), "42.0s");
  assert_eq!(duration(185_000), "3m05s");
  assert_eq!(duration(245_000), "4m05s");
  assert_eq!(duration(7_620_000), "2h07m");
  assert_eq!(duration(7_800_000), "2h10m");
  assert_eq!(duration(273_600_000), "3d04h");
}

#[test]
fn sizes() {
  assert_eq!(size(512.0), "512 B");
  assert_eq!(size(1536.0), "1.5 KB");
  assert_eq!(size(200.0 * 1024.0 * 1024.0), "200 MB");
  assert_eq!(size(3.2 * 1024.0 * 1024.0 * 1024.0), "3.2 GB");
  assert_eq!(size(5000.0 * 1024.0 * 1024.0 * 1024.0), "5000 GB");
}

#[test]
fn counts() {
  assert_eq!(count(0), "0");
  assert_eq!(count(987), "987");
  assert_eq!(count(14_987), "14,987");
  assert_eq!(count(1_204_331), "1,204,331");
}
