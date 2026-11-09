// Fetch assets: get chat's emotes and flairs, and their stylesheets, from
// the CDN and summarise them, checking the stylesheets against them.
//
// Usage: cargo run -p dgg-api --features http --example fetch_assets --
//          [cache key]

use {
  ::dgg_api::assets::{DEFAULT_CDN, css, fetch, flair},
  ::std::{collections::BTreeMap, env, process::ExitCode},
};

#[::tokio::main]
async fn main() -> ExitCode {
  let cache_key = env::args().nth(1);
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let client = ::reqwest::Client::new();
  let cache_key = cache_key.as_deref();
  let (emotes, flairs, emotes_css, flairs_css) = match ::tokio::try_join!(
    fetch::emotes(&client, DEFAULT_CDN, cache_key),
    fetch::flairs(&client, DEFAULT_CDN, cache_key),
    fetch::emotes_css(&client, DEFAULT_CDN, cache_key),
    fetch::flairs_css(&client, DEFAULT_CDN, cache_key),
  ) {
    Ok(all) => all,
    Err(e) => {
      eprintln!("{e}");
      return ExitCode::FAILURE;
    }
  };

  let mut tiers = BTreeMap::new();
  for emote in &emotes.value {
    *tiers.entry(emote.minimum_sub_tier).or_insert(0) += 1;
  }
  println!("{} emotes ({} bytes)", emotes.value.len(), emotes.raw.len());
  for (tier, count) in tiers {
    println!("  tier {tier}+: {count}");
  }
  println!("{} flairs ({} bytes)", flairs.value.len(), flairs.raw.len());
  for tier in 1..=5 {
    let name = flair::sub_tier(tier).unwrap_or_default();
    let label = flairs
      .value
      .iter()
      .find(|flair| flair.name == name)
      .map_or("missing", |flair| flair.label.as_str());
    println!("  tier {tier}: {name} {label:?}");
  }

  let emotes_split = css::emotes(&emotes_css, &emotes.value);
  let flairs_split = css::flairs(&flairs_css, &flairs.value);
  let (emotes_split, flairs_split) = match (emotes_split, flairs_split) {
    (Ok(emotes), Ok(flairs)) => (emotes, flairs),
    (Err(e), _) => {
      eprintln!("emotes.css doesn't match emotes.json: {e}");
      return ExitCode::FAILURE;
    }
    (_, Err(e)) => {
      eprintln!("flairs.css doesn't match flairs.json: {e}");
      return ExitCode::FAILURE;
    }
  };
  println!(
    "emotes.css ({} bytes) matches: {} bytes of hand-written styles",
    emotes_css.len(),
    emotes_split.styles.len()
  );
  println!(
    "flairs.css ({} bytes) matches: {} bytes of preamble, {} after",
    flairs_css.len(),
    flairs_split.preamble.len(),
    flairs_split.rest.len()
  );
  ExitCode::SUCCESS
}
