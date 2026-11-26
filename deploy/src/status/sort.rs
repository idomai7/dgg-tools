//! The orders `status --watch` sorts its boxes in: each box's keys, the
//! first its default, each sorting the most useful first (the busiest, the
//! newest or a name's A to Z first), or reversed. Ties keep the order the
//! key before had.

use {
  super::{Capture, Program, program_build, unhealthiest},
  ::std::cmp::Ordering,
};

/// The programs box's keys.
pub const PROGRAMS: [&str; 5] = ["name", "memory", "cpu", "build", "started"];
/// The captures box's keys: `health` its default, the unhealthy first,
/// then the busiest.
pub const CAPTURES: [&str; 5] =
  ["health", "load", "service", "channel", "started"];
/// The journal box's keys: `list` its default, dgg's channels in their
/// list's order, then Kick's.
pub const JOURNAL: [&str; 4] = ["list", "name", "size", "last write"];

/// A box's order: which of its keys, and whether reversed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Order {
  pub key: usize,
  pub reversed: bool,
}

impl Order {
  /// The next of `keys` keys, round to the first, not reversed.
  pub fn next(&mut self, keys: usize) {
    self.key = (self.key + 1) % keys;
    self.reversed = false;
  }

  /// How a box with `keys` says it: `by health`, or `by load, reversed`.
  pub fn describe(&self, keys: &[&str]) -> String {
    match self.reversed {
      false => format!("by {}", keys[self.key]),
      true => format!("by {}, reversed", keys[self.key]),
    }
  }

  fn apply(&self, ordering: Ordering) -> Ordering {
    match self.reversed {
      true => ordering.reverse(),
      false => ordering,
    }
  }
}

/// Sorts `programs`, by name and start as found, in `order`.
pub fn programs(programs: &mut [Program], order: Order) {
  programs.sort_by(|a, b| {
    order.apply(match PROGRAMS[order.key] {
      "name" => a.name.cmp(&b.name).then(a.started.cmp(&b.started)),
      "memory" => b.memory.cmp(&a.memory),
      "cpu" => b.cpu.total_cmp(&a.cpu),
      "build" => program_build(a).cmp(&program_build(b)),
      _ => b.started.cmp(&a.started),
    })
  });
}

/// Sorts `captures`, as found (the unhealthy first, then the busiest,
/// then dgg's before Kick's, each by channel), in `order`.
pub fn captures(captures: &mut [Capture], order: Order) {
  let per_minute = |c: &Capture| c.per_minute.unwrap_or(0.0);
  captures.sort_by(|a, b| {
    order.apply(match CAPTURES[order.key] {
      "health" => unhealthiest(a).cmp(&unhealthiest(b)),
      "load" => per_minute(b).total_cmp(&per_minute(a)),
      "service" => a.kick.cmp(&b.kick),
      "channel" => a.listening.cmp(&b.listening),
      _ => b.started.cmp(&a.started),
    })
  });
}

/// A channel in the journal box, as it's sorted.
pub struct Channel {
  pub name: String,
  /// Bytes written today.
  pub bytes: u64,
  /// Unix ms of the newest write today.
  pub last_write: Option<i64>,
}

/// Sorts `channels`, in the list's order as found, in `order`.
pub fn journal<T>(channels: &mut [(Channel, T)], order: Order) {
  if JOURNAL[order.key] == "list" {
    if order.reversed {
      channels.reverse();
    }
    return;
  }
  channels.sort_by(|(a, _), (b, _)| {
    order.apply(match JOURNAL[order.key] {
      "name" => a.name.cmp(&b.name),
      "size" => b.bytes.cmp(&a.bytes),
      _ => b.last_write.cmp(&a.last_write),
    })
  });
}
