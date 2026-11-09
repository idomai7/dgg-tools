//! Short human forms of durations and sizes.

/// `42s`, `3m 5s`, `2h 10m`, `3d 4h`.
pub fn age(ms: i64) -> String {
  let s = ms.max(0) / 1000;
  if s < 60 {
    return format!("{s}s");
  }
  let m = s / 60;
  if m < 60 {
    return format!("{m}m {}s", s % 60);
  }
  let h = m / 60;
  if h < 48 {
    return format!("{h}h {}m", m % 60);
  }
  format!("{}d {}h", h / 24, h % 24)
}

/// `512 B`, `1.5 KB`, `20 MB`, `3.2 GB`.
pub fn size(bytes: f64) -> String {
  let mut value = bytes;
  for unit in ["B", "KB", "MB", "GB"] {
    if value < 1024.0 || unit == "GB" {
      return if unit == "B" || value >= 100.0 {
        format!("{value:.0} {unit}")
      } else {
        format!("{value:.1} {unit}")
      };
    }
    value /= 1024.0;
  }
  unreachable!()
}
