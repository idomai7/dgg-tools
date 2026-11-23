//! What a moderator does from the page: delete a message, time out, ban
//! or unban a user, as Kick's official API lets the signed-in moderator.

use {
  ::kick_api::official::{Api, Error, api::Ban},
  ::serde::{Deserialize, Serialize},
};

/// The longest timeout Kick takes, in minutes: 7 days.
pub const MAX_TIMEOUT: u32 = 10080;
/// The longest reason Kick takes, in characters.
pub const MAX_REASON: usize = 100;

/// An action, as the page asks for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
  /// The channel's slug.
  pub channel: String,
  #[serde(flatten)]
  pub action: Action,
  /// Whom it's on.
  pub user: Target,
  /// The message it's taken from; the one deleted, for a delete.
  pub message_id: Option<String>,
  pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
  Delete,
  Timeout { minutes: u32 },
  Ban,
  Unban,
}

/// A user acted on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
  pub id: u64,
  pub username: String,
}

impl Asked {
  /// Whether Kick could take it: why not if not. A blank reason is none.
  pub fn check(mut self) -> Result<Self, String> {
    self.reason = self
      .reason
      .map(|reason| reason.trim().to_owned())
      .filter(|reason| !reason.is_empty());
    if let Some(reason) = &self.reason
      && reason.chars().count() > MAX_REASON
    {
      return Err(format!("a reason is at most {MAX_REASON} characters"));
    }
    match self.action {
      Action::Delete if self.message_id.is_none() => {
        Err("a delete needs the message's id".into())
      }
      Action::Timeout { minutes } if !(1..=MAX_TIMEOUT).contains(&minutes) => {
        Err(format!("a timeout is 1 to {MAX_TIMEOUT} minutes"))
      }
      _ => Ok(self),
    }
  }

  /// Asks Kick to do it, in the channel of `broadcaster_user_id`. It has
  /// to have been [checked](Self::check).
  pub async fn perform(
    &self,
    api: &Api,
    broadcaster_user_id: u64,
  ) -> Result<(), Error> {
    let ban = |duration| Ban {
      broadcaster_user_id,
      user_id: self.user.id,
      duration,
      reason: self.reason.clone(),
    };
    match self.action {
      Action::Delete => {
        let id = self.message_id.as_deref().expect("checked: a delete's id");
        api.delete_message(id).await
      }
      Action::Timeout { minutes } => api.ban(&ban(Some(minutes))).await,
      Action::Ban => api.ban(&ban(None)).await,
      Action::Unban => api.unban(broadcaster_user_id, self.user.id).await,
    }
  }
}
