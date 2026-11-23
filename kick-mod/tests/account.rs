//! Signing in, and the saved token, against a local server standing in for
//! Kick's. Ids, secrets and tokens are made up.

mod fake;

use {
  ::kick_api::official::oauth::App,
  ::kick_mod::account::{Account, SCOPES},
  ::reqwest::Url,
  ::std::{collections::HashMap, fs, path::PathBuf},
  fake::{Kick, TempDir},
};

const REDIRECT: &str = "http://localhost:8787/oauth/callback";

const TOKEN: &str = r#"{"access_token":"access-1","token_type":"Bearer",
  "refresh_token":"refresh-1","expires_in":3600,"scope":"user:read"}"#;
const ME: &str =
  r#"{"data":[{"user_id":7,"name":"moderator"}],"message":"OK"}"#;

/// Kick, signing in and naming the moderator, and where the token goes.
async fn kick() -> (Kick, TempDir, PathBuf) {
  let kick = Kick::start().await;
  kick.answer("/oauth/token", 200, TOKEN);
  kick.answer("/users", 200, ME);
  kick.answer("/oauth/revoke", 200, "OK");
  let dir = TempDir::new("account");
  let token = dir.0.join("kick-mod").join("token.json");
  (kick, dir, token)
}

fn account(kick: &Kick, token: &PathBuf) -> Account {
  let mut app = App::new("app-id", "app-secret");
  app.base = kick.base.clone();
  let http = ::reqwest::Client::new();
  Account::new(http, app, &kick.base, REDIRECT, token).unwrap()
}

fn query(url: &str) -> HashMap<String, String> {
  Url::parse(url)
    .unwrap()
    .query_pairs()
    .into_owned()
    .collect()
}

/// Signs in, as Kick's redirect back would.
async fn sign_in(account: &Account) {
  let state = &query(&account.start())["state"];
  let user = account.finish(&format!("code=code-1&state={state}")).await;
  assert_eq!(user.unwrap().user_id, 7);
}

#[tokio::test]
async fn signs_in() {
  let (kick, _dir, token) = kick().await;
  let account = account(&kick, &token);
  assert!(account.me().await.unwrap().is_none());
  assert!(account.api().is_none());

  let url = account.start();
  let asked = query(&url);
  assert!(url.starts_with(&format!("{}/oauth/authorize?", kick.base)));
  assert_eq!(asked["redirect_uri"], REDIRECT);
  assert_eq!(asked["scope"], SCOPES.join(" "));
  let state = &asked["state"];
  let user = account.finish(&format!("code=code-1&state={state}")).await;
  let user = user.unwrap();
  assert_eq!((user.user_id, user.name.as_deref()), (7, Some("moderator")));
  assert_eq!(kick.paths(), ["/oauth/token", "/users"]);
  assert!(kick.requests()[0].body.contains("code=code-1"));
  assert_eq!(kick.requests()[1].bearer.as_deref(), Some("access-1"));

  // Saved, and read back at the next start.
  assert!(fs::read_to_string(&token).unwrap().contains("refresh-1"));
  let again = self::account(&kick, &token);
  assert_eq!(again.me().await.unwrap().unwrap().user_id, 7);
}

#[tokio::test]
async fn only_its_own_sign_ins() {
  let (kick, _dir, token) = kick().await;
  let account = account(&kick, &token);
  let unknown = account.finish("code=code-1&state=made-up").await;
  assert!(unknown.unwrap_err().contains("isn't one kick-mod started"));
  assert!(account.finish("code=code-1").await.is_err());

  // Each sign-in finishes once.
  let state = query(&account.start())["state"].clone();
  let finish = format!("code=code-1&state={state}");
  assert!(account.finish(&finish).await.is_ok());
  assert!(account.finish(&finish).await.is_err());

  // Only the latest few are kept.
  let first = query(&account.start())["state"].clone();
  for _ in 0..8 {
    account.start();
  }
  let late = account.finish(&format!("code=c&state={first}")).await;
  assert!(late.unwrap_err().contains("too old"));
  assert!(kick.paths().iter().filter(|p| *p == "/oauth/token").count() == 1);
}

#[tokio::test]
async fn says_why_it_didnt_sign_in() {
  let (kick, _dir, token) = kick().await;
  let account = account(&kick, &token);
  let state = query(&account.start())["state"].clone();
  let refused = format!(
    "error=access_denied&error_description=The+user+said+no&state={state}"
  );
  assert_eq!(
    account.finish(&refused).await.unwrap_err(),
    "Kick refused: access_denied: The user said no"
  );

  kick.answer("/oauth/token", 400, r#"{"error":"invalid_grant"}"#);
  let state = query(&account.start())["state"].clone();
  let failed = account.finish(&format!("code=c&state={state}")).await;
  assert!(failed.unwrap_err().contains("invalid_grant"));
  assert!(account.api().is_none() && !token.exists());
}

#[tokio::test]
async fn a_token_that_cant_be_refreshed_signs_out() {
  let (kick, _dir, token) = kick().await;
  let account = account(&kick, &token);
  sign_in(&account).await;
  // Refused, and then the refresh refused too.
  kick.answer("/users", 401, r#"{"message":"Unauthorized"}"#);
  kick.answer("/oauth/token", 400, r#"{"error":"invalid_grant"}"#);
  assert!(account.me().await.unwrap().is_none());
  assert!(account.api().is_none() && !token.exists());
}

#[tokio::test]
async fn kick_failing_doesnt_sign_out() {
  let (kick, _dir, token) = kick().await;
  let account = account(&kick, &token);
  sign_in(&account).await;
  kick.answer("/users", 500, "down");
  assert_eq!(account.me().await.unwrap_err().status(), Some(500));
  assert!(account.api().is_some() && token.exists());
}

#[tokio::test]
async fn signs_out() {
  let (kick, _dir, token) = kick().await;
  let account = account(&kick, &token);
  sign_in(&account).await;
  account.sign_out().await;
  assert!(account.api().is_none() && !token.exists());
  assert!(account.me().await.unwrap().is_none());
  let revoked: Vec<_> = kick
    .requests()
    .into_iter()
    .filter(|r| r.path == "/oauth/revoke")
    .map(|r| r.query)
    .collect();
  assert_eq!(
    revoked,
    [
      "token=refresh-1&token_hint_type=refresh_token",
      "token=access-1&token_hint_type=access_token"
    ]
  );
  // Signing out again does nothing.
  account.sign_out().await;
}

#[tokio::test]
async fn a_broken_token_file_is_reported() {
  let (kick, dir, token) = kick().await;
  fs::create_dir_all(token.parent().unwrap()).unwrap();
  fs::write(&token, "{ not json").unwrap();
  let app = App::new("app-id", "app-secret");
  let http = ::reqwest::Client::new();
  let made = Account::new(http, app, &kick.base, REDIRECT, &token);
  assert!(made.is_err());
  drop(dir);
}
