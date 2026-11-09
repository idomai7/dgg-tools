// Fetch assets: get chat's emotes and flairs from the CDN and summarise
// them.
//
// Usage: cargo run -p dgg-api --features http --example fetch_assets --
//          [cache key]

use {
  ::dgg_api::assets::{DEFAULT_CDN, fetch, flair},
  ::std::{collections::BTreeMap, env, process::ExitCode},
};

#[::tokio::main]
async fn main() -> ExitCode {
  let cache_key = env::args().nth(1);
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let client = ::reqwest::Client::new();
  let cache_key = cache_key.as_deref();
  let (emotes, flairs) = match ::tokio::try_join!(
    fetch::emotes(&client, DEFAULT_CDN, cache_key),
    fetch::flairs(&client, DEFAULT_CDN, cache_key),
  ) {
    Ok(both) => both,
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
  ExitCode::SUCCESS
}
