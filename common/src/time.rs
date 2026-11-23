//! Time as the workspace keeps it: unix milliseconds, `i64`, in UTC. Days
//! are `YYYY-MM-DD`, also in UTC.
//!
//! - Now: [`now_ms`].
//! - Reading an RFC 3339 timestamp: [`unix_ms`].
//! - Writing one: [`rfc3339`], or for people, [`utc_timestamp`].
//! - The day a timestamp is in, and back: [`utc_day`] and [`day_start`].
//! - The calendar arithmetic under them, Howard Hinnant's:
//!   [`days_from_civil`] and [`civil_from_days`].

use ::std::time::{SystemTime, UNIX_EPOCH};

pub const DAY_MS: i64 = 86_400_000;

/// Now, in unix milliseconds.
pub fn now_ms() -> i64 {
  let since = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default();
  i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
}

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

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for unix ms `ms`: RFC 3339, as
/// [`unix_ms`] reads it.
pub fn rfc3339(ms: i64) -> String {
  format!("{}T{}Z", utc_day(ms), time_of_day(ms))
}

/// `YYYY-MM-DD HH:MM:SS.mmmZ` for unix ms `ms`, for people to read, as log
/// lines start.
pub fn utc_timestamp(ms: i64) -> String {
  format!("{} {}Z", utc_day(ms), time_of_day(ms))
}

/// `YYYY-MM-DD` of unix ms `ms`.
pub fn utc_day(ms: i64) -> String {
  let (year, month, day) = civil_from_days(ms.div_euclid(DAY_MS));
  format!("{year:04}-{month:02}-{day:02}")
}

/// Unix ms at the start of `day`, `YYYY-MM-DD`: `None` if it isn't one.
pub fn day_start(day: &str) -> Option<i64> {
  if day.len() != 10 {
    return None;
  }
  unix_ms(&format!("{day}T00:00:00Z"))
}

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
  let year = if month <= 2 { year - 1 } else { year };
  let era = year.div_euclid(400);
  let year_of_era = year - era * 400;
  let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
  let day_of_era =
    year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01: `(year, month, day)`.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
  let days = days + 719_468;
  let era = days.div_euclid(146_097);
  let day_of_era = days - era * 146_097;
  let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524
    - day_of_era / 146_096)
    / 365;
  let day_of_year =
    day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let shifted_month = (5 * day_of_year + 2) / 153;
  let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
  let month = if shifted_month < 10 {
    shifted_month + 3
  } else {
    shifted_month - 9
  };
  let year = year_of_era + era * 400 + i64::from(month <= 2);
  (year, month, day)
}

/// `HH:MM:SS.mmm` of unix ms `ms`.
fn time_of_day(ms: i64) -> String {
  let in_day = ms.rem_euclid(DAY_MS);
  let (hours, minutes) = (in_day / 3_600_000, in_day / 60_000 % 60);
  let (seconds, millis) = (in_day / 1000 % 60, in_day % 1000);
  format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

fn days_in_month(year: i64, month: i64) -> i64 {
  match month {
    2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
    2 => 28,
    4 | 6 | 9 | 11 => 30,
    _ => 31,
  }
}
