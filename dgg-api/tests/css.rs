use ::dgg_api::assets::{
  css::{self, Mismatch},
  emote, flair,
};

/// Cut from what the CDN served: the fixture emotes' and flairs' rules,
/// with the styles of the first few emotes.
const EMOTES_CSS: &str = include_str!("fixtures/assets/emotes.css");
const FLAIRS_CSS: &str = include_str!("fixtures/assets/flairs.css");
const EMOTES: &str = include_str!("fixtures/assets/emotes.json");
const FLAIRS: &str = include_str!("fixtures/assets/flairs.json");

#[test]
fn splits_off_the_emotes_styles() {
  let emotes = emote::parse(EMOTES).unwrap();
  let split = css::emotes(EMOTES_CSS, &emotes).unwrap();
  assert_eq!(split.preamble, "");
  assert!(split.styles.starts_with(".emote.ACKSHUALLY {"), "{split:?}");
  assert!(
    split
      .styles
      .trim_end()
      .ends_with("margin-right: -4px;  \n}")
  );
}

#[test]
fn finds_emotes_that_differ() {
  let mut emotes = emote::parse(EMOTES).unwrap();
  emotes[1].image[0].height += 1;
  let Err(Mismatch { offset, expected }) = css::emotes(EMOTES_CSS, &emotes)
  else {
    panic!()
  };
  assert!(EMOTES_CSS[offset..].starts_with(".emote.AbaW {"));
  assert!(expected.contains("height: 33px;"), "{expected}");
  // Out of order.
  emotes = emote::parse(EMOTES).unwrap();
  emotes.swap(0, 1);
  assert!(css::emotes(EMOTES_CSS, &emotes).is_err());
}

#[test]
fn splits_off_the_flairs_preamble() {
  let flairs = flair::parse(FLAIRS).unwrap();
  let split = css::flairs(FLAIRS_CSS, &flairs).unwrap();
  assert!(split.preamble.starts_with(":root {"));
  assert!(split.preamble.contains("@keyframes move"));
  assert!(!split.preamble.contains(".flair."));
  assert_eq!(split.rest, "");
}

#[test]
fn finds_flairs_that_differ() {
  // flair33 is shown and rainbow; bot shown and coloured; admin hidden.
  type Change = fn(&mut flair::Flair);
  let changes: [(&str, Change); 6] = [
    ("flair33", |flair| flair.hidden = true),
    ("flair33", |flair| flair.priority += 1),
    ("flair33", |flair| flair.rainbow_color = false),
    ("bot", |flair| flair.color = Some("#000000".into())),
    ("bot", |flair| flair.image[0].width += 1),
    ("admin", |flair| flair.hidden = false),
  ];
  for (name, change) in changes {
    let mut flairs = flair::parse(FLAIRS).unwrap();
    change(flairs.iter_mut().find(|f| f.name == name).unwrap());
    assert!(css::flairs(FLAIRS_CSS, &flairs).is_err(), "{name}");
  }
}
