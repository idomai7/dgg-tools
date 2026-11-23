//! The moderator's Kick account: signing in from the page, and the token,
//! kept in `data/kick-mod/token.json`.
//!
//! The page sends the moderator to [`Account::start`]'s URL, and Kick sends
//! them back to the redirect URI, `/oauth/callback`, whose query
//! [`Account::finish`] takes. The token is saved then, again on each
//! refresh, and read back at the next start. Once it can't be refreshed,
//! [`Account::me`] says nobody is signed in, and the page asks to sign in
//! again.

use {
  ::common::atomic_file,
  ::kick_api::official::{
    Api, Error,
    api::User,
    oauth::{App, Hint, Refused, SignIn, Token},
  },
  ::reqwest::Url,
  ::std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
  },
};

pub const DEFAULT_TOKEN_PATH: &str = "data/kick-mod/token.json";

/// What kick-mod asks the moderator to grant.
pub const SCOPES: &[&str] = &[
  "user:read",
  "channel:read",
  "chat:write",
  "moderation:ban",
  "moderation:chat_message:manage",
];

/// Sign-ins under way at once; beyond this, the oldest is dropped.
const PENDING: usize = 8;

pub struct Account {
  http: ::reqwest::Client,
  app: App,
  api_base: String,
  redirect_uri: String,
  token_path: PathBuf,
  pending: Mutex<VecDeque<SignIn>>,
  /// The signed-in moderator's, if any.
  api: RwLock<Option<Arc<Api>>>,
}

impl Account {
  /// Signed in with the token saved at `token_path`, if there is one.
  /// `redirect_uri` has to be one the app registered; `api_base` is
  /// [`kick_api::official::api::DEFAULT_BASE`] but in tests.
  pub fn new(
    http: ::reqwest::Client,
    app: App,
    api_base: impl Into<String>,
    redirect_uri: impl Into<String>,
    token_path: impl Into<PathBuf>,
  ) -> Result<Self, String> {
    let account = Self {
      http,
      app,
      api_base: api_base.into(),
      redirect_uri: redirect_uri.into(),
      token_path: token_path.into(),
      pending: Mutex::new(VecDeque::new()),
      api: RwLock::new(None),
    };
    let path = &account.token_path;
    let token = match fs::read_to_string(path) {
      Ok(saved) => Some(
        ::serde_json::from_str(&saved)
          .map_err(|e| format!("{}: {e}", path.display()))?,
      ),
      Err(e) if e.kind() == io::ErrorKind::NotFound => None,
      Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some(token) = token {
      *account.api.write().unwrap() = Some(account.user_api(token));
    }
    Ok(account)
  }

  /// Starts a sign-in: the URL to send the moderator to.
  pub fn start(&self) -> String {
    let sign_in = self.app.sign_in(&self.redirect_uri, SCOPES);
    let url = sign_in.url.clone();
    let mut pending = self.pending.lock().unwrap();
    pending.push_back(sign_in);
    if pending.len() > PENDING {
      pending.pop_front();
    }
    url
  }

  /// Finishes a sign-in with the query Kick redirected back with (without
  /// the `?`): the moderator who signed in.
  pub async fn finish(&self, query: &str) -> Result<User, String> {
    let sign_in = self.take_pending(query).ok_or(
      "this sign-in isn't one kick-mod started, or it's too old: sign in \
       again",
    )?;
    let code = sign_in.code(query).map_err(|refused| match refused {
      Refused::Error { error, description } => match description {
        Some(description) => format!("Kick refused: {error}: {description}"),
        None => format!("Kick refused: {error}"),
      },
      Refused::State => "this sign-in isn't kick-mod's".into(),
      Refused::NoCode => "Kick sent no code".into(),
    })?;
    let token = self
      .app
      .exchange(&self.http, &self.redirect_uri, &sign_in.verifier, &code)
      .await
      .map_err(|e| format!("Kick didn't give a token: {e}"))?;
    save(&self.token_path, &token)
      .map_err(|e| format!("{}: {e}", self.token_path.display()))?;
    let api = self.user_api(token);
    *self.api.write().unwrap() = Some(api.clone());
    match api.users(&[]).await {
      Ok(users) => users.into_iter().next().ok_or("Kick named no user".into()),
      Err(e) => Err(format!("signed in, but Kick didn't say who: {e}")),
    }
  }

  /// The signed-in moderator, asked of Kick: `None` if nobody is, or their
  /// token can't be refreshed any more, which signs them out.
  pub async fn me(&self) -> Result<Option<User>, Error> {
    let Some(api) = self.api() else {
      return Ok(None);
    };
    match api.users(&[]).await {
      Ok(users) => Ok(users.into_iter().next()),
      Err(e) if dead(&e) => {
        self.forget(&api);
        Ok(None)
      }
      Err(e) => Err(e),
    }
  }

  /// Signs out: the token is revoked with Kick, as far as Kick lets it,
  /// and forgotten here.
  pub async fn sign_out(&self) {
    let Some(api) = self.api() else {
      return;
    };
    self.forget(&api);
    let Some(token) = api.token().await else {
      return;
    };
    if let Some(refresh) = &token.refresh_token {
      let hint = Some(Hint::RefreshToken);
      let _ = self.app.revoke(&self.http, refresh, hint).await;
    }
    let hint = Some(Hint::AccessToken);
    let _ = self.app.revoke(&self.http, &token.access_token, hint).await;
  }

  /// The signed-in moderator's API, if anyone is signed in.
  pub fn api(&self) -> Option<Arc<Api>> {
    self.api.read().unwrap().clone()
  }

  fn user_api(&self, token: Token) -> Arc<Api> {
    let path = self.token_path.clone();
    let keep = move |token: &Token| {
      if let Err(e) = save(&path, token) {
        eprintln!("{}: the refreshed token wasn't saved: {e}", path.display());
      }
    };
    let api = Api::user(self.http.clone(), self.app.clone(), token, keep);
    Arc::new(api.with_base(&self.api_base))
  }

  /// The pending sign-in `query`'s `state` names, taken out.
  fn take_pending(&self, query: &str) -> Option<SignIn> {
    let url = Url::parse(&format!("http://redirect/?{query}")).ok()?;
    let state = url
      .query_pairs()
      .find(|(key, _)| key == "state")
      .map(|(_, state)| state.into_owned())?;
    let mut pending = self.pending.lock().unwrap();
    let at = pending.iter().position(|s| s.state == state)?;
    pending.remove(at)
  }

  /// Signs `api` out here, unless someone has signed in since.
  fn forget(&self, api: &Arc<Api>) {
    let mut current = self.api.write().unwrap();
    if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, api)) {
      *current = None;
      let _ = fs::remove_file(&self.token_path);
    }
  }
}

/// Whether `error` says the token no longer works: no refresh token, or
/// refused even after a refresh (a 401), or the refresh refused (Kick
/// answers an invalid grant with a 400).
fn dead(error: &Error) -> bool {
  matches!(error, Error::NoRefreshToken)
    || matches!(error.status(), Some(400 | 401))
}

fn save(path: &Path, token: &Token) -> io::Result<()> {
  atomic_file::replace(path, ::serde_json::to_string(token)?)
}
