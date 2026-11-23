//! `control/synced-archives`: the archives taken from peers.

use ::logger::taken::{LEGACY, VERSION, decode, encode};

#[test]
fn format() {
  let (a, b) = ("ab".repeat(32), "cd".repeat(32));
  let taken = vec![
    ("2026-11-07".to_owned(), a.clone()),
    ("2026-11-08".to_owned(), b.clone()),
  ];
  let text = encode(&taken);
  assert_eq!(
    text,
    format!(
      "synced-archives dgg.0.2\u{1e}\n2026-11-07 {a}\u{1e}\n2026-11-08 {b}\u{1e}\n"
    )
  );
  assert_eq!(decode(&text), Some((VERSION, taken.clone())));
  // Before headers: a line each.
  let legacy = format!("2026-11-07 {a}\n2026-11-08 {b}\n");
  assert_eq!(decode(&legacy), Some((LEGACY, taken)));
  assert_eq!(decode(""), Some((LEGACY, vec![])));
  for bad in [
    format!("synced-archives dgg.0.2\u{1e}\n2026-11-07 {a}\n"),
    "synced-archives dgg.0.2\n".to_owned(),
    "synced-archives 0.0\n".to_owned(),
  ] {
    assert_eq!(decode(&bad), None, "{bad:?}");
  }
}
