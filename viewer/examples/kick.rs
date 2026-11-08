// Probe: subscribe to a Kick channel's chat.message.sent webhook and print verified messages.
// Needs KICK_CLIENT_ID / KICK_CLIENT_SECRET in .env, and the app's webhook URL pointing at
// a tunnel to http://localhost:8787/webhook/kick.
// Usage: cargo run --example kick -- <slug>
use axum::{
  Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode,
  routing::post,
};
use base64::Engine;
use rsa::{
  RsaPublicKey,
  pkcs1v15::{Signature, VerifyingKey},
  pkcs8::DecodePublicKey,
  sha2::Sha256,
  signature::Verifier,
};
use serde_json::{Value, json};
use std::sync::Arc;

const API: &str = "https://api.kick.com/public/v1";

#[tokio::main]
async fn main() {
  let _ = rustls::crypto::ring::default_provider().install_default();
  dotenvy::dotenv().ok();
  let slug = std::env::args().nth(1).expect("usage: kick <slug>");
  let http = reqwest::Client::new();

  let token: Value = http
    .post("https://id.kick.com/oauth/token")
    .form(&[
      ("grant_type", "client_credentials"),
      (
        "client_id",
        &std::env::var("KICK_CLIENT_ID").expect("KICK_CLIENT_ID"),
      ),
      (
        "client_secret",
        &std::env::var("KICK_CLIENT_SECRET").expect("KICK_CLIENT_SECRET"),
      ),
    ])
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
  let token = token["access_token"]
    .as_str()
    .expect("no access_token")
    .to_owned();
  println!("got app token");

  let chan: Value = http
    .get(format!("{API}/channels"))
    .query(&[("slug", &slug)])
    .bearer_auth(&token)
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
  let broadcaster = chan["data"][0]["broadcaster_user_id"]
    .as_u64()
    .unwrap_or_else(|| panic!("channel lookup failed: {chan}"));
  println!("{slug} -> broadcaster_user_id {broadcaster}");

  let key: Value = http
    .get(format!("{API}/public-key"))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
  let pem = key["data"]["public_key"]
    .as_str()
    .unwrap_or_else(|| panic!("bad public-key response: {key}"));
  let verifier = Arc::new(VerifyingKey::<Sha256>::new(
    RsaPublicKey::from_public_key_pem(pem).unwrap(),
  ));

  let app = Router::new()
    .route("/webhook/kick", post(webhook))
    .with_state(verifier);
  let listener = tokio::net::TcpListener::bind("0.0.0.0:8787").await.unwrap();
  tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
  println!("listening on :8787");

  let sub = http
    .post(format!("{API}/events/subscriptions"))
    .bearer_auth(&token)
    .json(&json!({
        "events": [{"name": "chat.message.sent", "version": 1}],
        "method": "webhook",
        "broadcaster_user_id": broadcaster,
    }))
    .send()
    .await
    .unwrap();
  let status = sub.status();
  let sub: Value = sub.json().await.unwrap_or_default();
  println!("subscribe {status}: {sub}");

  tokio::signal::ctrl_c().await.unwrap();
  if let Some(id) = sub["data"][0]["subscription_id"].as_str() {
    let del = http
      .delete(format!("{API}/events/subscriptions"))
      .query(&[("id", id)])
      .bearer_auth(&token)
      .send()
      .await
      .unwrap();
    println!("unsubscribe {}", del.status());
  }
}

async fn webhook(
  State(verifier): State<Arc<VerifyingKey<Sha256>>>,
  headers: HeaderMap,
  body: Bytes,
) -> StatusCode {
  let h = |name: &str| {
    headers
      .get(name)
      .and_then(|v| v.to_str().ok())
      .unwrap_or("")
  };
  let signed = [
    h("Kick-Event-Message-Id").as_bytes(),
    b".",
    h("Kick-Event-Message-Timestamp").as_bytes(),
    b".",
    &body,
  ]
  .concat();
  let ok = base64::engine::general_purpose::STANDARD
    .decode(h("Kick-Event-Signature"))
    .ok()
    .and_then(|sig| Signature::try_from(sig.as_slice()).ok())
    .is_some_and(|sig| verifier.verify(&signed, &sig).is_ok());
  if !ok {
    println!("rejected webhook (bad signature)");
    return StatusCode::UNAUTHORIZED;
  }
  let msg: Value = serde_json::from_slice(&body).unwrap_or_default();
  println!(
    "[{}] {}: {}",
    h("Kick-Event-Type"),
    msg["sender"]["username"],
    msg["content"]
  );
  StatusCode::OK
}
