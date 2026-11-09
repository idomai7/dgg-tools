use ::dgg_api::assets::{
  self, Emote, Flair, emote,
  flair::{self, SUB_TIERS},
};

/// A few of each, from what the CDN served, creators anonymised.
const EMOTES: &str = include_str!("fixtures/assets/emotes.json");
const FLAIRS: &str = include_str!("fixtures/assets/flairs.json");

fn emote(prefix: &str) -> Emote {
  let emotes = emote::parse(EMOTES).unwrap();
  emotes.into_iter().find(|e| e.prefix == prefix).unwrap()
}

fn flairs() -> Vec<Flair> {
  flair::parse(FLAIRS).unwrap()
}

#[test]
fn urls() {
  assert_eq!(
    assets::emotes_url(assets::DEFAULT_CDN, None),
    "https://cdn.destiny.gg/emotes/emotes.json"
  );
  assert_eq!(
    assets::flairs_url("http://localhost/cdn/", Some("1794203923")),
    "http://localhost/cdn/flairs/flairs.json?_=1794203923"
  );
  assert_eq!(
    assets::emotes_css_url(assets::DEFAULT_CDN, Some("k")),
    "https://cdn.destiny.gg/emotes/emotes.css?_=k"
  );
  assert_eq!(
    assets::flairs_css_url(assets::DEFAULT_CDN, None),
    "https://cdn.destiny.gg/flairs/flairs.css"
  );
}

#[test]
fn emotes() {
  assert_eq!(emote::parse(EMOTES).unwrap().len(), 8);
  let abathur = emote("Abathur");
  // Credited with "": no creator.
  assert_eq!(abathur.creator, None);
  assert_eq!((abathur.minimum_sub_tier, abathur.patreon_only), (0, false));
  let image = abathur.image().unwrap();
  assert_eq!(
    (image.mime.as_str(), image.width, image.height),
    ("image/png", 82, 30)
  );
  assert!(image.url.starts_with("https://cdn.destiny.gg/"));
  assert_eq!(emote("DEATH").creator, None);
  assert_eq!(emote("ACKSHUALLY").creator.as_deref(), Some("user1"));
  assert_eq!(emote("ALARMA").image().unwrap().mime, "image/gif");
}

#[test]
fn who_can_use_an_emote() {
  let open = emote("Abathur");
  assert!(open.usable(0, false));
  let tiered = emote("BUTTSTINY");
  assert_eq!(tiered.minimum_sub_tier, 5);
  assert!(!tiered.usable(4, true));
  assert!(tiered.usable(5, false));
  // As chat-gui reads it: Patreon decides, not the tier.
  let patreon: Vec<Emote> = emote::parse(
    r#"[{"prefix":"Patron","minimumSubTier":0,"patreonOnly":true}]"#,
  )
  .unwrap();
  assert!(!patreon[0].usable(5, false));
  assert!(patreon[0].usable(0, true));
  assert!(patreon[0].image().is_none());
}

#[test]
fn flairs_decode() {
  let flairs = flairs();
  assert_eq!(flairs.len(), 13);
  let micro = &flairs[0];
  assert_eq!(
    (micro.name.as_str(), micro.label.as_str(), micro.priority),
    ("flair17", "Micro", 1)
  );
  assert_eq!(micro.color.as_deref(), Some("#FCE205"));
  assert!(micro.hidden && micro.description.is_some());
  let by_name = |name| flairs.iter().find(|f| f.name == name).unwrap();
  // An image entry of nulls: no picture.
  assert!(by_name("flair125").image().is_none());
  // Colour "": none. Description null: none.
  assert_eq!(by_name("protected").color, None);
  assert!(!by_name("protected").colors());
  assert_eq!(by_name("admin").description, None);
  assert!(by_name("flair42").rainbow_color);
}

#[test]
fn sub_tier_flairs() {
  assert_eq!(flair::sub_tier(0), None);
  assert_eq!(flair::sub_tier(1), Some("flair13"));
  assert_eq!(flair::sub_tier(5), Some("flair42"));
  assert_eq!(flair::sub_tier(6), None);
  let flairs = flairs();
  for (tier, name) in SUB_TIERS.iter().enumerate() {
    let flair = flairs.iter().find(|f| f.name == *name).unwrap();
    assert!(flair.label.ends_with(&format!("Tier {}", tier + 1)));
  }
}

#[test]
fn username_colors() {
  let flairs = flairs();
  let color = |features: &[&str]| {
    flair::username_color(&flairs, features).map(|f| f.name.as_str())
  };
  assert_eq!(color(&[]), None);
  // Protected has no colour; the moderator flair does.
  assert_eq!(color(&["protected", "moderator"]), Some("moderator"));
  // Lower priority wins, in any order.
  assert_eq!(color(&["subscriber", "flair13", "flair8"]), Some("flair8"));
  assert_eq!(color(&["unknown", "bot"]), Some("bot"));
}
