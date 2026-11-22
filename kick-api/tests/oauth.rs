//! Signing in and tokens, against a local server standing in for Kick's.
//! Ids, secrets and tokens are made up.

mod fake;

use {
  ::kick_api::official::{
    Error,
    oauth::{self, App, Hint, Introspection, Refused, SignIn, Token},
  },
  ::reqwest::Url,
  ::std::collections::HashMap,
};

const REDIRECT: &str = "http://localhost:8787/oauth/callback";

const USER_TOKEN: &str = r#"{"access_token":"access-1","token_type":"Bearer",
  "refresh_token":"refresh-1","expires_in":3600,
  "scope":"user:read moderation:ban"}"#;

/// An app whose sign-in is served by a local server that answers every
/// request with `status` and `body`.
async fn serve(status: u16, body: &'static str) -> (App, fake::Server) {
  let server = fake::serve(move |_| (status, body.into())).await;
  let mut app = App::new("app-id", "app-secret");
  app.base = server.base.clone();
  (app, server)
}

fn pairs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
  pairs.iter().map(|&(k, v)| (k.into(), v.into())).collect()
}

#[test]
fn challenge_is_s256() {
  // From Python: base64url(sha256(verifier)), unpadded.
  assert_eq!(
    oauth::challenge("dBjftJeZ4CVP-mJ92K9qpdf0VuS3AQlnmtt8sdXYQh8"),
    "RM2JZCBv6hBgshCYb9VOByDZACRB5uxQ04sUkEhgx74"
  );
}

#[test]
fn sign_in_url() {
  let app = App::new("app-id", "app-secret");
  let sign_in = app.sign_in(REDIRECT, &["user:read", "moderation:ban"]);
  let url = Url::parse(&sign_in.url).unwrap();
  assert_eq!(url.origin().unicode_serialization(), oauth::DEFAULT_BASE);
  assert_eq!(url.path(), "/oauth/authorize");
  let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
  assert_eq!(
    query,
    pairs(&[
      ("response_type", "code"),
      ("client_id", "app-id"),
      ("redirect_uri", REDIRECT),
      ("scope", "user:read moderation:ban"),
      ("state", &sign_in.state),
      ("code_challenge", &oauth::challenge(&sign_in.verifier)),
      ("code_challenge_method", "S256"),
    ])
  );
  // 16 and 32 random bytes; PKCE wants a verifier of 43 to 128.
  assert_eq!((sign_in.state.len(), sign_in.verifier.len()), (22, 43));
  let again = app.sign_in(REDIRECT, &[]);
  assert_ne!(
    (&again.state, &again.verifier),
    (&sign_in.state, &sign_in.verifier)
  );
  assert!(!format!("{app:?}").contains("app-secret"));
}

#[test]
fn code_from_the_redirect() {
  let sign_in = SignIn {
    url: String::new(),
    state: "s-1".into(),
    verifier: "v".into(),
  };
  assert_eq!(sign_in.code("code=c%2F1&state=s-1"), Ok("c/1".into()));
  assert_eq!(sign_in.code("state=s-1&code=c"), Ok("c".into()));
  assert_eq!(sign_in.code("code=c&state=s-2"), Err(Refused::State));
  assert_eq!(sign_in.code("code=c"), Err(Refused::State));
  assert_eq!(sign_in.code("state=s-1"), Err(Refused::NoCode));
  assert_eq!(
    sign_in.code("error=access_denied&error_description=The+user+said+no"),
    Err(Refused::Error {
      error: "access_denied".into(),
      description: Some("The user said no".into()),
    })
  );
}

#[test]
fn tokens() {
  let token = Token::decode(USER_TOKEN, 1_000).unwrap();
  assert_eq!(token.access_token, "access-1");
  assert_eq!(token.refresh_token.as_deref(), Some("refresh-1"));
  assert_eq!(token.expires_at, 3_601_000);
  assert_eq!(
    token.scopes().collect::<Vec<_>>(),
    ["user:read", "moderation:ban"]
  );
  let shown = format!("{token:?}");
  assert!(!shown.contains("access-1") && !shown.contains("refresh-1"));
  let kept = ::serde_json::to_string(&token).unwrap();
  assert_eq!(::serde_json::from_str::<Token>(&kept).unwrap(), token);

  let app = r#"{"access_token":"a","token_type":"Bearer","expires_in":60}"#;
  let app = Token::decode(app, 0).unwrap();
  assert_eq!((app.refresh_token, app.scope), (None, None));
  assert!(Token::decode(r#"{"expires_in":60}"#, 0).is_err());
  assert!(Token::decode(r#"{"access_token":"a"}"#, 0).is_err());
}

#[tokio::test]
async fn exchanges_the_code() {
  let (app, server) = serve(200, USER_TOKEN).await;
  let http = ::reqwest::Client::new();
  let token = app.exchange(&http, REDIRECT, "verifier-1", "code-1").await;
  assert_eq!(token.unwrap().access_token, "access-1");
  let [request] = &server.requests()[..] else {
    panic!("{:?}", server.requests());
  };
  assert_eq!((&*request.method, &*request.path), ("POST", "/oauth/token"));
  assert_eq!(
    request.headers["content-type"],
    "application/x-www-form-urlencoded"
  );
  assert_eq!(
    request.form(),
    pairs(&[
      ("grant_type", "authorization_code"),
      ("client_id", "app-id"),
      ("client_secret", "app-secret"),
      ("redirect_uri", REDIRECT),
      ("code_verifier", "verifier-1"),
      ("code", "code-1"),
    ])
  );
}

#[tokio::test]
async fn refreshes_and_app_tokens() {
  let (app, server) = serve(200, USER_TOKEN).await;
  let http = ::reqwest::Client::new();
  let token = app.refresh(&http, "refresh-0").await.unwrap();
  assert_eq!(token.refresh_token.as_deref(), Some("refresh-1"));
  assert_eq!(
    server.requests()[0].form(),
    pairs(&[
      ("grant_type", "refresh_token"),
      ("client_id", "app-id"),
      ("client_secret", "app-secret"),
      ("refresh_token", "refresh-0"),
    ])
  );

  let answer = r#"{"access_token":"app-1","expires_in":60}"#;
  let (app, server) = serve(200, answer).await;
  assert_eq!(app.app_token(&http).await.unwrap().access_token, "app-1");
  assert_eq!(
    server.requests()[0].form(),
    pairs(&[
      ("grant_type", "client_credentials"),
      ("client_id", "app-id"),
      ("client_secret", "app-secret"),
    ])
  );
}

#[tokio::test]
async fn revokes() {
  let (app, server) = serve(200, "OK").await;
  let http = ::reqwest::Client::new();
  app
    .revoke(&http, "refresh-1", Some(Hint::RefreshToken))
    .await
    .unwrap();
  let request = &server.requests()[0];
  assert_eq!(
    (&*request.method, &*request.path),
    ("POST", "/oauth/revoke")
  );
  assert_eq!(
    request.query,
    [
      ("token".into(), "refresh-1".into()),
      ("token_hint_type".into(), "refresh_token".into())
    ]
  );
  assert!(request.body.is_empty());
}

#[tokio::test]
async fn introspects() {
  let answer = r#"{"data":{"active":true,"client_id":"app-id",
    "token_type":"app","scope":"","exp":1700000000,"new":1},
    "message":"OK"}"#;
  let (app, server) = serve(200, answer).await;
  let http = ::reqwest::Client::new();
  let introspection = app.introspect(&http, "access-1").await.unwrap();
  assert_eq!(
    introspection,
    Introspection {
      active: true,
      client_id: Some("app-id".into()),
      token_type: Some("app".into()),
      scope: Some("".into()),
      exp: Some(1_700_000_000),
      rest: ::serde_json::from_str(r#"{"new":1}"#).unwrap(),
    }
  );
  let request = &server.requests()[0];
  assert_eq!(
    (&*request.method, &*request.path),
    ("POST", "/oauth/token/introspect")
  );
  assert_eq!(request.bearer(), Some("access-1"));

  let (app, _server) = serve(200, r#"{"data":{"active":false}}"#).await;
  let inactive = app.introspect(&http, "old").await.unwrap();
  assert!(!inactive.active && inactive.client_id.is_none());
}

#[tokio::test]
async fn keeps_what_failed() {
  let http = ::reqwest::Client::new();
  let refused = r#"{"error":"invalid_grant"}"#;
  let (app, _server) = serve(400, refused).await;
  match app.refresh(&http, "refresh-0").await {
    Err(Error::Status { status, body }) => {
      assert_eq!((status, &*body), (400, refused))
    }
    other => panic!("{other:?}"),
  }
  let (app, _server) = serve(200, "{}").await;
  match app.app_token(&http).await {
    Err(Error::Decode { raw, .. }) => assert_eq!(raw, "{}"),
    other => panic!("{other:?}"),
  }
}
