//! Kick's timestamps, RFC 3339 strings (`2026-11-19T12:01:56+00:00`, or
//! with `Z`), decoded into unix milliseconds by [`common::time`].

use {
  ::common::time::unix_ms,
  ::serde_json::{Map, Value},
};

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
