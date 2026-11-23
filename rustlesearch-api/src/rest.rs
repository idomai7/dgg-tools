//! The fields a decoded object had that its type doesn't name, kept rather
//! than dropped: a field rustlesearch.dev adds, or renames, lands here
//! instead of being lost.

use {
  ::serde::{Deserialize, Deserializer},
  ::serde_json::{Map, Value},
};

/// Fields an object had that its type doesn't name: `None` when there were
/// none, never an empty map.
pub type Rest = Option<Map<String, Value>>;

/// Decodes a `#[serde(flatten)]` [`Rest`]: whatever the object's named
/// fields left over.
pub fn deserialize<'de, D: Deserializer<'de>>(
  deserializer: D,
) -> Result<Rest, D::Error> {
  let rest = Map::deserialize(deserializer)?;
  Ok((!rest.is_empty()).then_some(rest))
}
