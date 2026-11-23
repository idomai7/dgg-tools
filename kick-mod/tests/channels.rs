//! The channels file. Slugs and ids are made up.

use {
  ::common::temp_dir::TempDir,
  ::kick_mod::channels::{self, Channel, Channels},
  ::serde_json::{Map, json},
  ::std::{fs, path::PathBuf},
};

/// `channels.json`, in a directory that doesn't exist yet.
fn file(dir: &TempDir) -> PathBuf {
  dir.path().join("kick-mod").join("channels.json")
}

fn channel(broadcaster_user_id: u64, chatroom_id: Option<u64>) -> Channel {
  Channel {
    broadcaster_user_id,
    chatroom_id,
    rest: Map::new(),
  }
}

#[test]
fn none_before_the_first_write() {
  let dir = TempDir::new("kick-mod-channels");
  assert_eq!(channels::load(&file(&dir)), Ok(Channels::new()));
}

#[test]
fn records_lookups() {
  let mut known = Channels::new();
  channels::record(&mut known, "one", 1, Some(10));
  channels::record(&mut known, "two", 2, None);
  assert_eq!(known["one"], channel(1, Some(10)));
  assert_eq!(known["two"], channel(2, None));
  // A chatroom id that wasn't found leaves the one entered by hand.
  known.get_mut("two").unwrap().chatroom_id = Some(20);
  channels::record(&mut known, "two", 2, None);
  assert_eq!(known["two"].chatroom_id, Some(20));
  // One that was found replaces it.
  channels::record(&mut known, "two", 3, Some(30));
  assert_eq!(known["two"], channel(3, Some(30)));
}

#[test]
fn writes_sorted_and_pretty() {
  let dir = TempDir::new("kick-mod-channels");
  let path = file(&dir);
  channels::update(&path, |known| {
    channels::record(known, "zed", 2, None);
    channels::record(known, "abe", 1, Some(10));
  })
  .unwrap();
  let written = fs::read_to_string(&path).unwrap();
  assert_eq!(
    written,
    "{\n  \"abe\": {\n    \"broadcaster_user_id\": 1,\n    \
     \"chatroom_id\": 10\n  },\n  \"zed\": {\n    \
     \"broadcaster_user_id\": 2,\n    \"chatroom_id\": null\n  }\n}\n"
  );
  // Nothing is left beside it.
  let beside: Vec<_> = fs::read_dir(path.parent().unwrap())
    .unwrap()
    .map(|entry| entry.unwrap().file_name())
    .collect();
  assert_eq!(beside, ["channels.json"]);
}

#[test]
fn keeps_hand_edits() {
  let dir = TempDir::new("kick-mod-channels");
  let path = file(&dir);
  channels::update(&path, |known| channels::record(known, "one", 1, None))
    .unwrap();
  // By hand, while kick-mod runs: a chatroom id, a field kick-mod doesn't
  // know, and a channel.
  let edited = json!({
    "one": { "broadcaster_user_id": 1, "chatroom_id": 10, "note": "mine" },
    "two": { "broadcaster_user_id": 2, "chatroom_id": 20 },
  });
  fs::write(&path, edited.to_string()).unwrap();
  let known = channels::update(&path, |known| {
    channels::record(known, "three", 3, Some(30))
  })
  .unwrap();
  assert_eq!(channels::load(&path).unwrap(), known);
  assert_eq!(known.len(), 3);
  assert_eq!(known["one"].chatroom_id, Some(10));
  assert_eq!(known["one"].rest["note"], "mine");
  assert_eq!(known["two"], channel(2, Some(20)));
}

#[test]
fn reports_a_broken_file() {
  let dir = TempDir::new("kick-mod-channels");
  let path = file(&dir);
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(&path, "{ not json").unwrap();
  assert!(channels::load(&path).is_err());
  // And doesn't write over it.
  assert!(channels::update(&path, |_| ()).is_err());
  assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
  let missing_id = r#"{"one": {"chatroom_id": 10}}"#;
  assert!(channels::parse(missing_id).is_err());
}
