//! Kick's timestamps, RFC 3339 strings (`2026-11-19T12:01:56+00:00`, or
//! with `Z`), as unix milliseconds, as `dgg-api` has its own.

use ::serde_json::{Map, Value};

/// `rfc3339` in unix milliseconds: `YYYY-MM-DDTHH:MM:SS`, an optional
/// fraction (cut to milliseconds), and `Z` or a `±HH:MM` offset. `None`
/// for anything else, leap seconds included: unix time has no room for
/// them.
pub fn unix_ms(rfc3339: &str) -> Option<i64> {
  let s = rfc3339.as_bytes();
  let number = |at: usize, len: usize| -> Option<i64> {
    let digits = s.get(at..at + len)?;
    digits.iter().try_fold(0, |n, &digit| {
      digit
        .is_ascii_digit()
        .then(|| n * 10 + i64::from(digit - b'0'))
    })
  };
  let separated = |at: usize, separator: &[u8]| {
    s.get(at).is_some_and(|byte| separator.contains(byte))
  };
  let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
  let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
  if !(separated(4, b"-")
    && separated(7, b"-")
    && separated(10, b"Tt")
    && separated(13, b":")
    && separated(16, b":"))
  {
    return None;
  }
  if !((1..=12).contains(&month)
    && (1..=days_in_month(year, month)).contains(&day)
    && hour < 24
    && minute < 60
    && second < 60)
  {
    return None;
  }
  let mut at = 19;
  let mut ms = 0;
  if separated(at, b".") {
    at += 1;
    let digits = s[at..].iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
      return None;
    }
    ms = number(at, digits.min(3))? * 10_i64.pow(3 - digits.min(3) as u32);
    at += digits;
  }
  let offset = match s.get(at..)? {
    b"Z" | b"z" => 0,
    [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
      let (hours, minutes) = (number(at + 1, 2)?, number(at + 4, 2)?);
      if hours > 23 || minutes > 59 {
        return None;
      }
      let offset = hours * 60 + minutes;
      if *sign == b'-' { -offset } else { offset }
    }
    _ => return None,
  };
  let days = days_from_civil(year, month, day);
  let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
  Some((seconds - offset * 60) * 1_000 + ms)
}

/// Takes `key` out of `rest` as a timestamp, if it's one: a string that
/// parses, or `null`. Anything else stays in `rest`, so it isn't lost.
pub(crate) fn take(
  rest: &mut Option<Map<String, Value>>,
  key: &str,
) -> Option<i64> {
  let map = rest.as_mut()?;
  let taken = match map.get(key)? {
    Value::Null => None,
    Value::String(s) => Some(unix_ms(s)?),
    _ => return None,
  };
  map.remove(key);
  if map.is_empty() {
    *rest = None;
  }
  taken
}

fn days_in_month(year: i64, month: i64) -> i64 {
  match month {
    2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
    2 => 28,
    4 | 6 | 9 | 11 => 30,
    _ => 31,
  }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
  let year = if month <= 2 { year - 1 } else { year };
  let era = year.div_euclid(400);
  let year_of_era = year - era * 400;
  let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
  let day_of_era =
    year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  era * 146_097 + day_of_era - 719_468
}
