//! A user's history, from a journal and a mod log made up here. Slugs, ids
//! and names are made up.

use {
  ::common::temp_dir::TempDir,
  ::kick_mod::{
    actions::{Action, Asked, Target},
    chat::{Named, What},
    history::{self, Deletion, MAX_MESSAGES},
    modlog::{Answer, Entry, Moderator},
  },
  ::logger_journal::{
    layout::{channel_root, live_files},
    record::kick::v0::Body,
    writer::kick::Writer,
  },
  ::serde_json::{Value, json},
  ::std::{fs, io::Write, path::Path},
};

const SLUG: &str = "somechannel";
const USER: u64 = 42;
/// 2024-03-01T00:00:00Z.
const DAY: i64 = 1_709_251_200_000;
const ENDPOINT: &str = "wss://example.com/connection";

/// A frame pushing `event` with `data`, as Kick's Centrifugo sends one.
fn frame(event: &str, data: Value) -> Body {
  let push = json!({
    "push": {
      "channel": "chatrooms.1000.v2",
      "pub": {
        "data": {
          "event": format!(r"App\Events\{event}"),
          "data": data.to_string(),
        },
      },
    },
  });
  Body::Frame(push.to_string())
}

fn message(id: &str, sender: u64, username: &str, content: &str) -> Body {
  let sender = json!({"id": sender, "username": username, "slug": username});
  let data = json!({"id": id, "content": content, "sender": sender});
  frame("ChatMessageEvent", data)
}

fn user(id: u64, username: &str) -> Value {
  json!({"id": id, "username": username, "slug": username})
}

/// Writes `records`, each at its receive time, to the channel's journal
/// under `journal`, as `instance`.
fn journal(journal: &Path, instance: &str, records: Vec<(i64, Body)>) {
  let root = channel_root(journal, SLUG);
  let mut writer = Writer::new(root, instance, SLUG);
  writer.write(DAY, 1, Body::Open(ENDPOINT.into())).unwrap();
  for (receive_time, body) in records {
    writer.write(receive_time, 1, body).unwrap();
  }
}

fn entry(at: i64, channel: &str, user_id: u64, action: Action) -> Entry {
  Entry {
    at,
    by: Moderator {
      user_id: 7,
      name: Some("moderator".into()),
    },
    broadcaster_user_id: 456,
    asked: Asked {
      channel: channel.into(),
      action,
      user: Target {
        id: user_id,
        username: format!("user{user_id}"),
      },
      message_id: None,
      reason: Some("why".into()),
    },
    answer: Answer::Done,
  }
}

fn modlog(entries: &[Entry]) -> String {
  entries
    .iter()
    .map(|entry| ::serde_json::to_string(entry).unwrap() + "\n")
    .collect()
}

#[test]
fn messages_events_and_actions() {
  let dir = TempDir::new("kick-mod-history");
  let root = dir.path();
  journal(
    root,
    "kick-mod-1",
    vec![
      (DAY + 1, message("m1", USER, "oldname", "hello")),
      (DAY + 2, message("m2", 9, "someoneelse", "not theirs")),
      (DAY + 3, message("m3", USER, "oldname", "deleted later")),
      (
        DAY + 4,
        frame(
          "MessageDeletedEvent",
          json!({
            "id": "d1", "message": {"id": "m3"}, "aiModerated": true,
          }),
        ),
      ),
      (
        DAY + 5,
        frame(
          "UserBannedEvent",
          json!({
            "id": "b1", "user": user(USER, "oldname"),
            "banned_by": user(0, "moderator"), "permanent": false, "duration": 10,
          }),
        ),
      ),
      (
        DAY + 6,
        frame(
          "UserBannedEvent",
          json!({
            "id": "b2", "user": user(9, "someoneelse"), "permanent": true,
          }),
        ),
      ),
    ],
  );
  // A later run, the next day, after they renamed.
  journal(
    root,
    "kick-mod-2",
    vec![
      (
        DAY + 86_400_000,
        frame(
          "UserUnbannedEvent",
          json!({
            "id": "u1", "user": user(USER, "newname"),
            "unbanned_by": user(0, "moderator"),
          }),
        ),
      ),
      (DAY + 86_400_001, message("m4", USER, "newname", "back")),
    ],
  );
  let log = modlog(&[
    entry(DAY + 5, SLUG, USER, Action::Timeout { minutes: 10 }),
    entry(DAY + 6, SLUG, 9, Action::Ban),
    entry(DAY + 7, "otherchannel", USER, Action::Ban),
  ]);

  let history = history::read(root, &log, SLUG, USER).unwrap();
  assert_eq!(history.user_id, USER);
  assert_eq!(history.username.as_deref(), Some("newname"));
  let said: Vec<_> = history
    .messages
    .iter()
    .map(|said| (said.received, said.id.as_str(), said.content.as_str()))
    .collect();
  assert_eq!(
    said,
    [
      (DAY + 1, "m1", "hello"),
      (DAY + 3, "m3", "deleted later"),
      (DAY + 86_400_001, "m4", "back"),
    ]
  );
  let deleted: Vec<_> = history.messages.iter().map(|s| s.deleted).collect();
  assert_eq!(
    deleted,
    [
      None,
      Some(Deletion {
        received: DAY + 4,
        ai: true
      }),
      None
    ]
  );
  assert_eq!(history.earlier, 0);
  let events: Vec<_> = history
    .events
    .iter()
    .map(|happened| (happened.received, happened.what.clone()))
    .collect();
  assert_eq!(
    events,
    [
      (
        DAY + 5,
        What::Banned {
          user: Named {
            id: USER,
            username: "oldname".into()
          },
          by: Some("moderator".into()),
          permanent: false,
          minutes: Some(10),
          until: None,
        }
      ),
      (
        DAY + 86_400_000,
        What::Unbanned {
          user: Named {
            id: USER,
            username: "newname".into()
          },
          by: Some("moderator".into()),
        }
      ),
    ]
  );
  // Only theirs, in this channel.
  assert_eq!(
    history.actions,
    [entry(DAY + 5, SLUG, USER, Action::Timeout { minutes: 10 })]
  );
  assert!(history.unreadable.is_empty());
}

#[test]
fn only_the_newest_messages() {
  let dir = TempDir::new("kick-mod-history");
  let extra = 2;
  let messages = (0..MAX_MESSAGES + extra)
    .map(|n| {
      let id = format!("m{n}");
      (DAY + n as i64, message(&id, USER, "someone", &id))
    })
    .collect();
  journal(dir.path(), "kick-mod-1", messages);
  let history = history::read(dir.path(), "", SLUG, USER).unwrap();
  assert_eq!(history.earlier, extra);
  assert_eq!(history.messages.len(), MAX_MESSAGES);
  assert_eq!(history.messages[0].id, format!("m{extra}"));
}

#[test]
fn from_the_mod_log_alone() {
  let dir = TempDir::new("kick-mod-history");
  let log = modlog(&[entry(DAY, SLUG, USER, Action::Ban)]);
  let history = history::read(dir.path(), &log, SLUG, USER).unwrap();
  assert_eq!(history.username.as_deref(), Some("user42"));
  assert_eq!(history.actions.len(), 1);
  assert!(history.messages.is_empty() && history.events.is_empty());

  // A slug that can't be a directory's name isn't looked for.
  let history = history::read(dir.path(), "", "../up", USER).unwrap();
  assert_eq!(history.username, None);

  let bad = history::read(dir.path(), "not json\n", SLUG, USER);
  assert!(bad.unwrap_err().starts_with("mod log: line 1:"));
}

#[test]
fn skips_what_it_cant_read() {
  let dir = TempDir::new("kick-mod-history");
  journal(
    dir.path(),
    "kick-mod-1",
    vec![(DAY + 1, message("m1", USER, "someone", "hello"))],
  );
  let files = live_files(&channel_root(dir.path(), SLUG)).unwrap();
  // A torn record at the tail, still being written.
  let mut file = fs::OpenOptions::new().append(true).open(&files[0]).unwrap();
  file.write_all(b"1709251200002 kick-mod-1 1 fra").unwrap();
  // And a file that isn't a journal.
  let other = files[0].with_file_name("other.journal");
  fs::write(&other, "something else\n").unwrap();

  let history = history::read(dir.path(), "", SLUG, USER).unwrap();
  assert_eq!(history.messages.len(), 1);
  assert_eq!(
    history.unreadable,
    [format!("{}: not a journal", other.display())]
  );
}

/// A history page of `messages`, newest first, as Kick serves it.
fn page(messages: &[Value]) -> String {
  json!({
    "data": {"messages": messages, "cursor": "1", "pinned_message": null},
    "message": "OK",
  })
  .to_string()
}

fn history_message(id: &str, sender: u64, username: &str, at: i64) -> Value {
  json!({
    "id": id,
    "chat_id": 1001,
    "user_id": sender,
    "content": format!("said {id}"),
    "type": "message",
    "metadata": "{}",
    "sender": {"id": sender, "username": username, "slug": username},
    "created_at": ::common::time::rfc3339(at),
    "thread_parent_id": null,
  })
}

#[test]
fn with_the_messages_a_gap_was_filled_with() {
  let dir = TempDir::new("kick-mod-history");
  let filled = page(&[
    history_message("m2", USER, "someone", DAY + 2_000),
    history_message("m3", 9, "someoneelse", DAY + 2_000),
    // Received before the gap too.
    history_message("m1", USER, "someone", DAY),
  ]);
  journal(
    dir.path(),
    "kick-mod-1",
    vec![
      (DAY + 1, message("m1", USER, "someone", "said m1")),
      (
        DAY + 4_000,
        Body::History {
          cursor: None,
          page: filled,
        },
      ),
      // A page that doesn't decode is skipped.
      (
        DAY + 4_001,
        Body::History {
          cursor: Some("1".into()),
          page: "<html>".into(),
        },
      ),
      (DAY + 5_000, message("m4", USER, "someone", "said m4")),
      (
        DAY + 6_000,
        frame(
          "MessageDeletedEvent",
          json!({
            "id": "d1", "message": {"id": "m2"},
          }),
        ),
      ),
    ],
  );

  let history = history::read(dir.path(), "", SLUG, USER).unwrap();
  let said: Vec<_> = history
    .messages
    .iter()
    .map(|said| (said.id.as_str(), said.recovered, said.deleted.is_some()))
    .collect();
  assert_eq!(
    said,
    [
      ("m1", false, false),
      ("m2", true, true),
      ("m4", false, false),
    ]
  );
  let m2 = &history.messages[1];
  assert_eq!((m2.received, m2.at), (DAY + 4_000, Some(DAY + 2_000)));
  assert_eq!(m2.content, "said m2");
  assert!(history.unreadable.is_empty());
}
