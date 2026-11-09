//! Gaps: stretches of time when no capture instance was connected.
//!
//! Each connection covers from its `open` to its `close`, or to its last
//! line if it never closed (its instance crashed). The union of those spans
//! is what was captured live; the holes between them are gaps.
//!
//! A gap may still have been filled: the connection that ended it replays
//! dgg's recent `HISTORY`. If the oldest message in that replay is no newer
//! than the gap's start, the gap is covered. Otherwise backfill may have
//! fetched it: a gap inside the `fetchrange` spans is backfilled.
//!
//! Gap bounds are local receive times and message timestamps are dgg's, so
//! coverage is only as exact as the two clocks agree.

use {
  ::logger_journal::layout::utc_day,
  ::rusqlite::{Connection, params},
};

/// A stretch with no connection open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
  /// When the last covering connection ended, unix ms.
  pub start: i64,
  /// When the next connection opened, unix ms.
  pub end: i64,
  /// The connection that ended the gap.
  pub resumed_by: (String, u64),
  /// Oldest message timestamp in that connection's HISTORY, if it had one.
  pub history_from: Option<i64>,
  /// Indexed messages timestamped inside the gap (so, from HISTORY or
  /// other sources).
  pub recovered: u64,
  /// Whether fetched spans from backfill sources cover the whole gap.
  pub backfilled: bool,
}

impl Gap {
  /// Whether the resuming connection's HISTORY reached back to the gap's
  /// start, so every message in it was replayed.
  pub fn covered(&self) -> bool {
    self.history_from.is_some_and(|from| from <= self.start)
  }

  /// Whether nothing is missing from the gap, by HISTORY or backfill.
  pub fn filled(&self) -> bool {
    self.covered() || self.backfilled
  }

  pub fn duration_ms(&self) -> i64 {
    self.end - self.start
  }
}

/// What the index says was captured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
  /// When the first connection opened, unix ms.
  pub first: i64,
  /// When the last connection ended (or was last seen), unix ms.
  pub last: i64,
  pub gaps: Vec<Gap>,
}

/// The coverage of every connection in the index, or `None` if there are
/// none.
pub fn coverage(db: &Connection) -> ::rusqlite::Result<Option<Coverage>> {
  let mut query = db.prepare(
    "SELECT instance, connection, opened, coalesce(closed, last_seen),
        history_from
      FROM connections ORDER BY opened",
  )?;
  let spans = query.query_map([], |row| {
    Ok((
      (row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64),
      row.get::<_, i64>(2)?,
      row.get::<_, i64>(3)?,
      row.get::<_, Option<i64>>(4)?,
    ))
  })?;
  let mut coverage: Option<Coverage> = None;
  for span in spans {
    let (connection, opened, ended, history_from) = span?;
    let Some(coverage) = coverage.as_mut() else {
      coverage = Some(Coverage {
        first: opened,
        last: ended,
        gaps: Vec::new(),
      });
      continue;
    };
    if opened > coverage.last {
      coverage.gaps.push(Gap {
        start: coverage.last,
        end: opened,
        resumed_by: connection,
        history_from,
        recovered: 0,
        backfilled: false,
      });
    }
    coverage.last = coverage.last.max(ended);
  }
  if let Some(coverage) = coverage.as_mut() {
    let fetched = fetched_spans(db, None)?;
    for gap in &mut coverage.gaps {
      gap.backfilled = fetched
        .iter()
        .any(|&(start, end)| start <= gap.start && gap.end <= end);
      gap.recovered = db.query_row(
        "SELECT count(*) FROM messages WHERE timestamp > ?1 AND timestamp < ?2",
        params![gap.start, gap.end],
        |row| row.get::<_, i64>(0),
      )? as u64;
    }
  }
  Ok(coverage)
}

/// The parts of `[from, to)` that `source` hasn't fetched yet, newest first:
/// what's left to do of a history import, however often it was stopped.
pub fn unfetched(
  db: &Connection,
  source: &str,
  from: i64,
  to: i64,
) -> ::rusqlite::Result<Vec<(i64, i64)>> {
  let mut left = Vec::new();
  let mut start = from;
  for (span_start, span_end) in fetched_spans(db, Some(source))? {
    if span_end <= start {
      continue;
    }
    if span_start >= to {
      break;
    }
    if span_start > start {
      left.push((start, span_start));
    }
    start = start.max(span_end);
  }
  if start < to {
    left.push((start, to));
  }
  left.reverse();
  Ok(left)
}

/// The fetched spans of `source`, or of every source, merged where they
/// touch or overlap, oldest first.
fn fetched_spans(
  db: &Connection,
  source: Option<&str>,
) -> ::rusqlite::Result<Vec<(i64, i64)>> {
  let mut query = db.prepare(
    "SELECT start_time, end_time FROM fetch_ranges
      WHERE ?1 IS NULL OR source = ?1
      ORDER BY start_time",
  )?;
  let mut merged: Vec<(i64, i64)> = Vec::new();
  let spans =
    query.query_map([source], |row| Ok((row.get(0)?, row.get(1)?)))?;
  for span in spans {
    let (start, end) = span?;
    match merged.last_mut() {
      Some(last) if start <= last.1 => last.1 = last.1.max(end),
      _ => merged.push((start, end)),
    }
  }
  Ok(merged)
}

/// `YYYY-MM-DD HH:MM:SS.mmm` in UTC.
pub fn format_utc(ms: i64) -> String {
  let in_day = ms.rem_euclid(86_400_000);
  let (hours, minutes) = (in_day / 3_600_000, in_day / 60_000 % 60);
  let (seconds, millis) = (in_day / 1000 % 60, in_day % 1000);
  format!(
    "{} {hours:02}:{minutes:02}:{seconds:02}.{millis:03}",
    utc_day(ms)
  )
}

/// A short human duration: `850ms`, `12.3s`, `4m05s`, `2h07m`, `3d04h`.
pub fn format_duration(ms: i64) -> String {
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
