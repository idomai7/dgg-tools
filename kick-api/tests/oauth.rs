//! Signing in and tokens, against a local server standing in for Kick's.
//! Ids, secrets and tokens are made up.

use {
  ::kick_api::official::{
    Error,
    oauth::{self, App, Hint, Refused, SignIn, Token},
  },
  ::reqwest::Url,
  ::std::collections::HashMap,
  ::tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
  },
};

const REDIRECT: &str = "http://localhost:8787/oauth/callback";

/// A request, as the server read it.
#[derive(Debug)]
struct Request {
  method: String,
  path: String,
  query: HashMap<String, String>,
  content_type: Option<String>,
  form: HashMap<String, String>,
}

/// An app whose sign-in is served locally, and the server, which answers
/// one request with `status` and `body` and returns what it was sent.
async fn serve(
  status: u16,
  body: &'static str,
) -> (App, ::tokio::task::JoinHandle<Request>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let mut app = App::new("app-id", "app-secret");
  app.base = format!("http://{}", listener.local_addr().unwrap());
  let server = ::tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut read = Vec::new();
    let head = loop {
      let mut chunk = [0; 4096];
      let n = socket.read(&mut chunk).await.unwrap();
      read.extend_from_slice(&chunk[..n]);
      if let Some(end) = read.windows(4).position(|w| w == b"\r\n\r\n") {
        break end;
      }
    };
    let text = String::from_utf8(read[..head].to_vec()).unwrap();
    let mut lines = text.split("\r\n");
    let mut start = lines.next().unwrap().split(' ');
    let (method, target) = (start.next().unwrap(), start.next().unwrap());
    let headers: HashMap<_, _> = lines
      .filter_map(|line| line.split_once(": "))
      .map(|(name, value)| (name.to_lowercase(), value.to_owned()))
      .collect();
    let length = headers
      .get("content-length")
      .map_or(0, |length| length.parse().unwrap());
    while read.len() < head + 4 + length {
      let mut chunk = [0; 4096];
      let n = socket.read(&mut chunk).await.unwrap();
      read.extend_from_slice(&chunk[..n]);
    }
    let reason = if status == 200 { "OK" } else { "Error" };
    let answer = format!(
      "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\n\
       connection: close\r\n\r\n{body}",
      body.len()
    );
    socket.write_all(answer.as_bytes()).await.unwrap();
    let url = Url::parse(&format!("http://server{target}")).unwrap();
    let form = Url::parse(&format!(
      "http://server/?{}",
      String::from_utf8_lossy(&read[head + 4..])
    ))
    .unwrap();
    Request {
      method: method.into(),
      path: url.path().into(),
      query: url.query_pairs().into_owned().collect(),
      content_type: headers.get("content-type").cloned(),
      form: form.query_pairs().into_owned().collect(),
    }
  });
  (app, server)
}

fn pairs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
  pairs.iter().map(|&(k, v)| (k.into(), v.into())).collect()
}

const USER_TOKEN: &str = r#"{"access_token":"access-1","token_type":"Bearer",
  "refresh_token":"refresh-1","expires_in":3600,
  "scope":"user:read moderation:ban"}"#;

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
  let request = server.await.unwrap();
  assert_eq!(token.unwrap().access_token, "access-1");
  assert_eq!((&*request.method, &*request.path), ("POST", "/oauth/token"));
  assert_eq!(
    request.content_type.as_deref(),
    Some("application/x-www-form-urlencoded")
  );
  assert_eq!(
    request.form,
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
    server.await.unwrap().form,
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
    server.await.unwrap().form,
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
  let request = server.await.unwrap();
  assert_eq!(
    (&*request.method, &*request.path),
    ("POST", "/oauth/revoke")
  );
  assert_eq!(
    request.query,
    pairs(&[("token", "refresh-1"), ("token_hint_type", "refresh_token")])
  );
  assert!(request.form.is_empty());
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
