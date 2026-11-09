use ::dgg_api::chat::protocol::vestaboard::{Color, Tile};

#[test]
fn maps_codes_as_chat_gui_does() {
  let text: String = (1..=60)
    .map(|code| match Tile::from_code(code) {
      Tile::Char(ch) => ch,
      Tile::Unknown(_) => '_',
      other => panic!("{code}: {other:?}"),
    })
    .collect();
  assert_eq!(
    text,
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890!@#$()_-_+&=;:_'\"%,.__/?"
  );
  assert_eq!(Tile::from_code(0), Tile::Blank);
  assert_eq!(Tile::from_code(63), Tile::Color(Color::Red));
  assert_eq!(Tile::from_code(71), Tile::Color(Color::Filled));
  for code in [61, 62, 72, 255] {
    assert_eq!(Tile::from_code(code), Tile::Unknown(code));
  }
}
