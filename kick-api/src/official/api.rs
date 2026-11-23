//! The public API's endpoints, behind [`Api`], which keeps its token.
//!
//! `Api` isn't a service: it does nothing between calls. A call refreshes
//! the token first if it expires within a minute, and once after a 401,
//! both through the same path as [`Api::refresh`], which the caller may
//! also use: to keep the token alive, or to check a saved one.

use {
  super::{
    Error,
    oauth::{App, Token},
    send,
  },
  crate::{
    rest::{self, Rest},
    time,
  },
  ::reqwest::{Method, RequestBuilder},
  ::serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned},
  ::tokio::sync::{Mutex, MutexGuard},
};

/// Where the public API is served.
pub const DEFAULT_BASE: &str = "https://api.kick.com/public/v1";

/// A token expiring within this long (ms) is refreshed before a call.
const MARGIN: i64 = 60_000;

/// Whose token the calls are made with.
enum Owner {
  /// A user's, with the callback that keeps each new one.
  User(Box<dyn Fn(&Token) + Send + Sync>),
  /// The app's own, fetched when needed.
  App,
}

/// The public API, as a user or as the app.
pub struct Api {
  http: ::reqwest::Client,
  app: App,
  base: String,
  owner: Owner,
  /// Held while refreshing, so calls at once share one refresh.
  token: Mutex<Option<Token>>,
}

impl Api {
  /// As the user `token` belongs to. `keep` is given each new token, to
  /// save in place of the last: the refresh token in it replaces the old.
  pub fn user(
    http: ::reqwest::Client,
    app: App,
    token: Token,
    keep: impl Fn(&Token) + Send + Sync + 'static,
  ) -> Self {
    Self {
      http,
      app,
      base: DEFAULT_BASE.into(),
      owner: Owner::User(Box::new(keep)),
      token: Mutex::new(Some(token)),
    }
  }

  /// As the app, with its own token, fetched on the first call.
  pub fn app(http: ::reqwest::Client, app: App) -> Self {
    Self {
      http,
      app,
      base: DEFAULT_BASE.into(),
      owner: Owner::App,
      token: Mutex::new(None),
    }
  }

  /// Served at `base` instead of [`DEFAULT_BASE`], as in tests.
  pub fn with_base(mut self, base: impl Into<String>) -> Self {
    self.base = base.into();
    self
  }

  /// The current token: `None` for the app's before the first call.
  pub async fn token(&self) -> Option<Token> {
    self.token.lock().await.clone()
  }

  /// A new token now, however long this one has left: a user's refreshed
  /// (and given to `keep`), or the app's fetched again. A failure isn't
  /// retried, and leaves the token as it was.
  pub async fn refresh(&self) -> Result<Token, Error> {
    let mut token = self.token.lock().await;
    self.renew(&mut token).await
  }

  /// `GET /users`: the users with `ids`, or with none, the signed-in user.
  pub async fn users(&self, ids: &[u64]) -> Result<Vec<User>, Error> {
    let query: Vec<_> = ids.iter().map(|id| ("id", id)).collect();
    self
      .data(Method::GET, "/users", |request| request.query(&query))
      .await
  }

  /// `GET /channels` by slug, at most 50: with none, the signed-in user's.
  pub async fn channels_by_slug(
    &self,
    slugs: &[&str],
  ) -> Result<Vec<Channel>, Error> {
    let query: Vec<_> = slugs.iter().map(|slug| ("slug", slug)).collect();
    self
      .data(Method::GET, "/channels", |request| request.query(&query))
      .await
  }

  /// `GET /channels` by broadcaster, at most 50: with none, the signed-in
  /// user's.
  pub async fn channels_by_id(
    &self,
    broadcaster_user_ids: &[u64],
  ) -> Result<Vec<Channel>, Error> {
    let query: Vec<_> = broadcaster_user_ids
      .iter()
      .map(|id| ("broadcaster_user_id", id))
      .collect();
    self
      .data(Method::GET, "/channels", |request| request.query(&query))
      .await
  }

  /// `POST /chat`.
  pub async fn send_chat(&self, chat: &Chat) -> Result<Sent, Error> {
    self
      .data(Method::POST, "/chat", |request| request.json(chat))
      .await
  }

  /// `DELETE /chat/<message_id>`.
  pub async fn delete_message(&self, message_id: &str) -> Result<(), Error> {
    let path = format!("/chat/{message_id}");
    self.call(Method::DELETE, &path, |request| request).await?;
    Ok(())
  }

  /// `POST /moderation/bans`: a ban, or with a duration, a timeout.
  pub async fn ban(&self, ban: &Ban) -> Result<(), Error> {
    let path = "/moderation/bans";
    self
      .call(Method::POST, path, |request| request.json(ban))
      .await?;
    Ok(())
  }

  /// `DELETE /moderation/bans`: lifts a ban or a timeout.
  pub async fn unban(
    &self,
    broadcaster_user_id: u64,
    user_id: u64,
  ) -> Result<(), Error> {
    let body = Unban {
      broadcaster_user_id,
      user_id,
    };
    let path = "/moderation/bans";
    self
      .call(Method::DELETE, path, |request| request.json(&body))
      .await?;
    Ok(())
  }

  /// A call whose answer is `{data, message}`: its `data`, decoded.
  async fn data<T: DeserializeOwned>(
    &self,
    method: Method,
    path: &str,
    build: impl Fn(RequestBuilder) -> RequestBuilder,
  ) -> Result<T, Error> {
    #[derive(Deserialize)]
    struct Answer<T> {
      data: T,
    }
    let raw = self.call(method, path, build).await?;
    match ::serde_json::from_str::<Answer<T>>(&raw) {
      Ok(answer) => Ok(answer.data),
      Err(error) => Err(Error::Decode {
        raw,
        error: error.to_string(),
      }),
    }
  }

  /// Sends a request with the current token, refreshed first if it's about
  /// to expire, and again once if it's refused with a 401.
  async fn call(
    &self,
    method: Method,
    path: &str,
    build: impl Fn(RequestBuilder) -> RequestBuilder,
  ) -> Result<String, Error> {
    let url = format!("{}{path}", self.base);
    let request = |access: &str| {
      build(self.http.request(method.clone(), &url)).bearer_auth(access)
    };
    let access = self.access().await?;
    match send(request(&access)).await {
      Err(Error::Status { status: 401, .. }) => {
        let mut token = self.token.lock().await;
        // Unless a call at the same time has refreshed it already.
        let current = token.as_ref().map(|token| &token.access_token);
        let access = if current == Some(&access) {
          self.renew(&mut token).await?.access_token
        } else {
          current.cloned().unwrap_or_default()
        };
        drop(token);
        send(request(&access)).await
      }
      answer => answer,
    }
  }

  /// The access token to call with, refreshed first if it's about to
  /// expire.
  async fn access(&self) -> Result<String, Error> {
    let mut token = self.token.lock().await;
    let fresh = token
      .as_ref()
      .is_some_and(|token| token.expires_at - time::now_ms() > MARGIN);
    if fresh {
      Ok(token.as_ref().unwrap().access_token.clone())
    } else {
      Ok(self.renew(&mut token).await?.access_token)
    }
  }

  /// The one path to a new token.
  async fn renew(
    &self,
    token: &mut MutexGuard<'_, Option<Token>>,
  ) -> Result<Token, Error> {
    let new = match &self.owner {
      Owner::App => self.app.app_token(&self.http).await?,
      Owner::User(keep) => {
        let old = token.as_ref().and_then(|token| token.refresh_token.clone());
        let old = old.ok_or(Error::NoRefreshToken)?;
        let mut new = self.app.refresh(&self.http, &old).await?;
        // Kick has always sent one; if it didn't, the old one is all there
        // is.
        new.refresh_token.get_or_insert(old);
        keep(&new);
        new
      }
    };
    **token = Some(new.clone());
    Ok(new)
  }
}

/// A user, from `GET /users`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct User {
  pub user_id: u64,
  pub name: Option<String>,
  /// Only for the signed-in user, with `user:read`.
  pub email: Option<String>,
  pub profile_picture: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A channel, from `GET /channels`. It has no channel or chatroom id: see
/// [`channel`](crate::channel).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Channel {
  pub broadcaster_user_id: u64,
  pub slug: Option<String>,
  pub stream_title: Option<String>,
  pub channel_description: Option<String>,
  pub banner_picture: Option<String>,
  pub category: Option<Category>,
  pub stream: Option<Stream>,
  pub active_subscribers_count: Option<u64>,
  pub active_gifted_subscribers_count: Option<u64>,
  pub canceled_subscribers_count: Option<u64>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Category {
  pub id: Option<u64>,
  pub name: Option<String>,
  pub thumbnail: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A channel's stream, live or the last one.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(remote = "Self")]
pub struct Stream {
  pub is_live: Option<bool>,
  pub is_mature: Option<bool>,
  pub language: Option<String>,
  /// Unix ms. A string that isn't RFC 3339 stays in `rest`.
  #[serde(skip)]
  pub start_time: Option<i64>,
  pub thumbnail: Option<String>,
  pub viewer_count: Option<u64>,
  pub custom_tags: Option<Vec<String>>,
  /// The stream's ingest, only for the channel's own broadcaster with
  /// `streamkey:read`.
  pub url: Option<String>,
  pub key: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

impl<'de> Deserialize<'de> for Stream {
  fn deserialize<D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Self, D::Error> {
    let mut decoded = Self::deserialize(deserializer)?;
    decoded.start_time = time::take(&mut decoded.rest, "start_time");
    Ok(decoded)
  }
}

/// A chat message to send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Chat {
  /// At most 500 characters (grapheme clusters) and 2048 bytes of UTF-8.
  pub content: String,
  pub r#type: Sender,
  /// Whose chat: needed as a user, ignored as a bot.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub broadcaster_user_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub reply_to_message_id: Option<String>,
}

/// Who a message is sent as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sender {
  /// The signed-in user, in `broadcaster_user_id`'s chat.
  User,
  /// The app's bot.
  Bot,
}

/// What `POST /chat` answers.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Sent {
  pub is_sent: bool,
  pub message_id: Option<String>,
  #[serde(flatten, deserialize_with = "rest::deserialize")]
  pub rest: Rest,
}

/// A ban, or with a `duration`, a timeout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ban {
  pub broadcaster_user_id: u64,
  pub user_id: u64,
  /// Minutes, 1 to 10080; `None` bans for good.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub duration: Option<u32>,
  /// At most 100 characters.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
}

#[derive(Serialize)]
struct Unban {
  broadcaster_user_id: u64,
  user_id: u64,
}
