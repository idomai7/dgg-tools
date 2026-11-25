//! An install's channel lists, written and converted. The channels and ids
//! are made up.

use {
  ::common::temp_dir::TempDir,
  ::deploy::lists::{retire_plain, write_missing},
  ::logger::{dgg, kick},
  ::std::fs,
};

#[test]
fn writes_the_lists_that_are_missing() {
  let root = TempDir::new("deploy-lists-new");
  let did = write_missing(root.path()).unwrap();
  assert_eq!(
    did,
    [
      "dgg-channels.jsonc: written",
      "kick-channels.jsonc: written"
    ]
  );
  let dgg = root.path().join(dgg::channels::FILE);
  let kick = root.path().join(kick::channels::FILE);
  assert_eq!(
    dgg::channels::load(&dgg).unwrap(),
    [dgg::channels::Channel::destiny()]
  );
  assert_eq!(kick::channels::load(&kick).unwrap(), []);

  // Once there, left as they are.
  fs::write(&kick, "edited").unwrap();
  assert_eq!(write_missing(root.path()).unwrap(), Vec::<String>::new());
  assert_eq!(fs::read_to_string(&kick).unwrap(), "edited");
  assert_eq!(retire_plain(root.path()).unwrap(), Vec::<String>::new());
}

#[test]
fn converts_the_plain_lists_and_retires_them() {
  let root = TempDir::new("deploy-lists-plain");
  root.write("dgg-channels", "a.example wss://a.example/ws -\n");
  root.write("kick-channels", "# mine\none 2 1\n");
  let did = write_missing(root.path()).unwrap();
  assert_eq!(
    did,
    [
      "dgg-channels.jsonc: converted from dgg-channels",
      "kick-channels.jsonc: converted from kick-channels",
    ]
  );
  let dgg = dgg::channels::load(&root.path().join(dgg::channels::FILE));
  assert_eq!(dgg.unwrap()[0].name, "a.example");
  let kick = kick::channels::load(&root.path().join(kick::channels::FILE));
  assert_eq!(kick.unwrap()[0].slug, "one");
  // Left for what was installed, which reads them, until it's stopped.
  assert!(root.path().join("kick-channels").exists());

  let did = retire_plain(root.path()).unwrap();
  assert_eq!(
    did,
    [
      "dgg-channels: renamed to dgg-channels.old, as dgg-channels.jsonc has \
       it",
      "kick-channels: renamed to kick-channels.old, as kick-channels.jsonc \
       has it",
    ]
  );
  assert!(!root.path().join("kick-channels").exists());
  let old = fs::read_to_string(root.path().join("kick-channels.old"));
  assert_eq!(old.unwrap(), "# mine\none 2 1\n");
}

#[test]
fn leaves_a_broken_plain_list_for_its_owner() {
  let root = TempDir::new("deploy-lists-broken");
  root.write("kick-channels", "one 2\n");
  let e = write_missing(root.path()).unwrap_err();
  assert!(e.contains("kick-channels: line 1"), "{e}");
  assert!(!root.path().join(kick::channels::FILE).exists());
  assert!(root.path().join("kick-channels").exists());
}
