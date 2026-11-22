//! What the examples that sign in share.

use {
  ::kick_api::{
    official::oauth::Token,
    realtime::{
      Client, Event, Item,
      centrifugo::{push, reply},
    },
  },
  ::std::{fs, time::Duration},
  ::tokio::time::timeout,
};

/// How long to wait for each event in realtime chat.
const WAIT: Duration = Duration::from_secs(30);

/// Saves `token` to `path`, as the examples read it.
pub fn save(path: &str, token: &Token) {
  fs::write(path, ::serde_json::to_string(token).unwrap()).unwrap();
}

/// Waits up to 30 s for the first item `found` picks out.
pub async fn wait<T>(
  client: &mut Client,
  mut found: impl FnMut(Item) -> Option<T>,
) -> Option<T> {
  let waiting = async {
    loop {
      let item = client.next().await.expect("realtime chat stopped");
      if let Some(found) = found(item) {
        return found;
      }
    }
  };
  timeout(WAIT, waiting).await.ok()
}

/// The event in a frame's first publication, if any.
pub fn event(item: Item) -> Option<Event> {
  let Item::Frame(frame) = item else {
    return None;
  };
  frame.replies().find_map(|reply| {
    let push = reply.ok()?.body?;
    let reply::Body::Push(push) = push else {
      return None;
    };
    let push::Body::Publication(publication) = push.body? else {
      return None;
    };
    Event::from_publication(&publication.data)
  })
}
