//! Signing in to Kick, and the tokens it gives: an app's own
//! (`client_credentials`), and a user's (an authorization code with PKCE),
//! refreshed and revoked.
//!
//! There's no server here: the tool serves the redirect URI, and hands the
//! query it's called with to [`SignIn::code`].

use {
  super::{Error, now, send},
  crate::rest::{self, Rest},
  ::base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
  ::reqwest::{Url, header::CONTENT_TYPE},
  ::serde::{Deserialize, Serialize},
  ::sha2::{Digest, Sha256},
  ::std::fmt,
};

/// Where Kick's sign-in is served.
pub const DEFAULT_BASE: &str = "https://id.kick.com";

/// An app, as registered on kick.com: its id and secret.
#[derive(Clone)]
pub struct App {
  pub client_id: String,
  pub client_secret: String,
  /// Where its sign-in is served: [`DEFAULT_BASE`] but in tests.
  pub base: String,
}

/// The secret isn't shown.
impl fmt::Debug for App {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.debug_struct("App")
      .field("client_id", &self.client_id)
      .field("base", &self.base)
      .finish_non_exhaustive()
  }
}

/// A sign-in under way: the URL to send the user to, and what to keep
/// until they're redirected back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignIn {
  pub url: String,
  /// Comes back with the code, to tell this sign-in's from another's.
  pub state: String,
  /// PKCE's code verifier: the code is exchanged with it.
  pub verifier: String,
}

/// Why the redirect didn't bring a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
  /// The user, or Kick, refused: OAuth's `error` and `error_description`.
  Error {
    error: String,
    description: Option<String>,
  },
  /// The `state` isn't this sign-in's.
  State,
  /// Neither a code nor an error.
  NoCode,
}

/// Which kind of token is revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
  AccessToken,
  RefreshToken,
}

/// What Kick says of a token, from [`App::introspect`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Introspection {
  /// When `false`, the rest is missing.
  pub active: bool,
  /// The app the token was given to.
  pub client_id: Option<String>,
  /// `app` or `user`.
  pub token_type: Option<String>,
  /// The scopes granted, separated by spaces.
  pub scope: Option<String>,
  /// When it expires, as Kick gives it.
  pub exp: Option<i64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A token, as Kick gave it, with when it expires. It serializes, so the
/// tool can keep it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
  pub access_token: String,
  /// A user token's; an app token has none.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub refresh_token: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub token_type: Option<String>,
  /// The scopes granted, separated by spaces.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub scope: Option<String>,
  /// Unix milliseconds: when it was received plus Kick's `expires_in`.
  pub expires_at: i64,
}

/// The tokens themselves aren't shown.
impl fmt::Debug for Token {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.debug_struct("Token")
      .field("token_type", &self.token_type)
      .field("scope", &self.scope)
      .field("expires_at", &self.expires_at)
      .finish_non_exhaustive()
  }
}

impl Token {
  /// The scopes granted.
  pub fn scopes(&self) -> impl Iterator<Item = &str> {
    self.scope.as_deref().unwrap_or("").split_whitespace()
  }

  /// Decodes Kick's answer, received at `now` (unix milliseconds).
  pub fn decode(json: &str, now: i64) -> Result<Self, String> {
    #[derive(Deserialize)]
    struct Answer {
      access_token: String,
      refresh_token: Option<String>,
      token_type: Option<String>,
      scope: Option<String>,
      expires_in: i64,
    }
    let answer: Answer =
      ::serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(Self {
      access_token: answer.access_token,
      refresh_token: answer.refresh_token,
      token_type: answer.token_type,
      scope: answer.scope,
      expires_at: now.saturating_add(answer.expires_in.saturating_mul(1_000)),
    })
  }
}

impl App {
  pub fn new(
    client_id: impl Into<String>,
    client_secret: impl Into<String>,
  ) -> Self {
    Self {
      client_id: client_id.into(),
      client_secret: client_secret.into(),
      base: DEFAULT_BASE.into(),
    }
  }

  /// Starts a user's sign-in: Kick asks them to grant `scopes`, then sends
  /// them to `redirect_uri`, which has to be one the app registered.
  pub fn sign_in(&self, redirect_uri: &str, scopes: &[&str]) -> SignIn {
    let (state, verifier) = (random(16), random(32));
    let url = Url::parse_with_params(
      &format!("{}/oauth/authorize", self.base),
      [
        ("response_type", "code"),
        ("client_id", &self.client_id),
        ("redirect_uri", redirect_uri),
        ("scope", &scopes.join(" ")),
        ("state", &state),
        ("code_challenge", &challenge(&verifier)),
        ("code_challenge_method", "S256"),
      ],
    )
    .expect("the base is a URL");
    SignIn {
      url: url.into(),
      state,
      verifier,
    }
  }

  /// Exchanges a sign-in's code for the user's token.
  pub async fn exchange(
    &self,
    http: &::reqwest::Client,
    redirect_uri: &str,
    verifier: &str,
    code: &str,
  ) -> Result<Token, Error> {
    let grant = [
      ("grant_type", "authorization_code"),
      ("redirect_uri", redirect_uri),
      ("code_verifier", verifier),
      ("code", code),
    ];
    self.token(http, &grant).await
  }

  /// A new token for a user's refresh token. Kick answers with a new
  /// refresh token too, to keep in place of this one.
  pub async fn refresh(
    &self,
    http: &::reqwest::Client,
    refresh_token: &str,
  ) -> Result<Token, Error> {
    let grant = [
      ("grant_type", "refresh_token"),
      ("refresh_token", refresh_token),
    ];
    self.token(http, &grant).await
  }

  /// The app's own token, for what needs no user.
  pub async fn app_token(
    &self,
    http: &::reqwest::Client,
  ) -> Result<Token, Error> {
    self
      .token(http, &[("grant_type", "client_credentials")])
      .await
  }

  /// Revokes a token. Kick takes it in the query.
  pub async fn revoke(
    &self,
    http: &::reqwest::Client,
    token: &str,
    hint: Option<Hint>,
  ) -> Result<(), Error> {
    let mut query = vec![("token", token)];
    if let Some(hint) = hint {
      let hint = match hint {
        Hint::AccessToken => "access_token",
        Hint::RefreshToken => "refresh_token",
      };
      query.push(("token_hint_type", hint));
    }
    let request = http
      .post(format!("{}/oauth/revoke", self.base))
      .query(&query)
      .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
    send(request).await.map(drop)
  }

  /// What Kick says of an access token, the app's or a user's.
  pub async fn introspect(
    &self,
    http: &::reqwest::Client,
    access_token: &str,
  ) -> Result<Introspection, Error> {
    #[derive(Deserialize)]
    struct Answer {
      data: Introspection,
    }
    let request = http
      .post(format!("{}/oauth/token/introspect", self.base))
      .bearer_auth(access_token);
    let raw = send(request).await?;
    match ::serde_json::from_str::<Answer>(&raw) {
      Ok(answer) => Ok(answer.data),
      Err(error) => Err(Error::Decode {
        raw,
        error: error.to_string(),
      }),
    }
  }

  async fn token(
    &self,
    http: &::reqwest::Client,
    grant: &[(&str, &str)],
  ) -> Result<Token, Error> {
    let mut form = vec![
      ("client_id", self.client_id.as_str()),
      ("client_secret", self.client_secret.as_str()),
    ];
    form.extend_from_slice(grant);
    let request = http.post(format!("{}/oauth/token", self.base)).form(&form);
    let raw = send(request).await?;
    Token::decode(&raw, now()).map_err(|error| Error::Decode { raw, error })
  }
}

impl SignIn {
  /// The code from the query the redirect URI was called with (without the
  /// `?`), if it's this sign-in's.
  pub fn code(&self, query: &str) -> Result<String, Refused> {
    let url = Url::parse(&format!("http://redirect/?{query}"))
      .map_err(|_| Refused::NoCode)?;
    let (mut code, mut state, mut error, mut description) =
      (None, None, None, None);
    for (key, value) in url.query_pairs() {
      let value = Some(value.into_owned());
      match &*key {
        "code" => code = value,
        "state" => state = value,
        "error" => error = value,
        "error_description" => description = value,
        _ => {}
      }
    }
    if let Some(error) = error {
      return Err(Refused::Error { error, description });
    }
    if state.as_deref() != Some(&*self.state) {
      return Err(Refused::State);
    }
    code.ok_or(Refused::NoCode)
  }
}

/// PKCE's `S256` challenge for `verifier`.
pub fn challenge(verifier: &str) -> String {
  URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `bytes` random bytes, in base64url.
fn random(bytes: usize) -> String {
  let mut random = vec![0; bytes];
  ::getrandom::fill(&mut random).expect("the OS has randomness");
  URL_SAFE_NO_PAD.encode(random)
}
