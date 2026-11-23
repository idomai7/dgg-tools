//! Randomness: its shapes, not its quality.

use {
  ::common::random::{backoff, fill, fraction, token, uuid},
  ::std::{collections::HashSet, time::Duration},
};

#[test]
fn fills_from_the_os() {
  let (mut a, mut b) = ([0; 32], [0; 32]);
  fill(&mut a);
  fill(&mut b);
  assert_ne!(a, b);
}

#[test]
fn tokens_are_unpadded_base64url() {
  // 16 and 32 bytes: 22 and 43 characters.
  assert_eq!((token(16).len(), token(32).len()), (22, 43));
  let url_safe = |b: u8| b.is_ascii_alphanumeric() || b"-_".contains(&b);
  assert!(token(32).bytes().all(url_safe));
  let tokens: HashSet<_> = (0..100).map(|_| token(16)).collect();
  assert_eq!(tokens.len(), 100);
}

#[test]
fn uuids_are_version_4() {
  let id = uuid();
  let parts: Vec<_> = id.split('-').map(str::len).collect();
  assert_eq!(parts, [8, 4, 4, 4, 12]);
  assert!(id.bytes().all(|b| b == b'-' || b.is_ascii_hexdigit()));
  assert_eq!(id.as_bytes()[14], b'4');
  assert!(b"89ab".contains(&id.as_bytes()[19]));
  assert_ne!(uuid(), id);
}

#[test]
fn fractions_are_in_range() {
  let all: Vec<_> = (0..1000).map(|_| fraction()).collect();
  assert!(all.iter().all(|f| (0.0..1.0).contains(f)));
  // Spread out, not stuck.
  assert!(all.iter().any(|&f| f < 0.25) && all.iter().any(|&f| f > 0.75));
}

#[test]
fn backoff_doubles_up_to_its_bound() {
  let (min, max) = (Duration::from_secs(1), Duration::from_secs(60));
  for step in [0, 1, 2, 5, 10, 31, 100] {
    for _ in 0..100 {
      let wait = backoff(step, min, max);
      let ceiling = min.saturating_mul(1 << step.min(31)).min(max);
      assert!(wait >= min && wait <= (min + ceiling).min(max), "{wait:?}");
    }
  }
  // Jittered: not the same every time.
  let waits: HashSet<_> = (0..20).map(|_| backoff(3, min, max)).collect();
  assert!(waits.len() > 1);
}
