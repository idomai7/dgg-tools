//! Short forms of durations, sizes and counts, for people to read.

/// `850ms`, `12.3s`, `4m05s`, `2h07m`, `3d04h`, for `ms` milliseconds. Less
/// than nothing, as clocks that disagree can give, is `0ms`.
pub fn duration(ms: i64) -> String {
  let ms = ms.max(0);
  let seconds = ms / 1000;
  if ms < 1000 {
    format!("{ms}ms")
  } else if ms < 60_000 {
    format!("{seconds}.{}s", ms % 1000 / 100)
  } else if ms < 3_600_000 {
    format!("{}m{:02}s", seconds / 60, seconds % 60)
  } else if ms < 86_400_000 {
    format!("{}h{:02}m", seconds / 3600, seconds / 60 % 60)
  } else {
    format!("{}d{:02}h", seconds / 86_400, seconds / 3600 % 24)
  }
}

/// `512 B`, `1.5 KB`, `20 MB`, `3.2 GB`, in units of 1024.
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

/// `987`, `14,987`, `1,204,331`.
pub fn count(n: u64) -> String {
  let digits = n.to_string();
  let mut out = String::new();
  for (index, digit) in digits.chars().enumerate() {
    if index > 0 && (digits.len() - index).is_multiple_of(3) {
      out.push(',');
    }
    out.push(digit);
  }
  out
}
