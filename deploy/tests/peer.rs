use {
  ::deploy::peer::{self, Peer},
  ::std::{fs, path::PathBuf},
};

fn file(name: &str, text: &str) -> PathBuf {
  let path = ::std::env::temp_dir()
    .join(format!("deploy-peer-{name}-{}.env", ::std::process::id()));
  fs::write(&path, text).unwrap();
  path
}

#[test]
fn reads_a_peer_file() {
  let path = file(
    "ok",
    "# comment\n\nLOGGER_PEER_URL = 'http://host:8788'\n\
     LOGGER_PEER_TOKEN=abc\nOTHER=1\n",
  );
  assert_eq!(
    peer::read(&path).unwrap(),
    Some(Peer {
      url: "http://host:8788".into(),
      token: "abc".into(),
    })
  );
  fs::remove_file(path).unwrap();
}

#[test]
fn no_file_is_no_peer() {
  let path = ::std::env::temp_dir().join("deploy-peer-missing.env");
  assert_eq!(peer::read(&path).unwrap(), None);
}

#[test]
fn rejects_incomplete_files() {
  for (name, text) in [
    ("no-token", "LOGGER_PEER_URL=http://host\n"),
    (
      "empty-token",
      "LOGGER_PEER_URL=http://host\nLOGGER_PEER_TOKEN=\n",
    ),
    ("not-key-value", "LOGGER_PEER_URL http://host\n"),
  ] {
    let path = file(name, text);
    let error = peer::read(&path).unwrap_err();
    // Never the token itself.
    assert!(!error.to_string().contains("abc"), "{error}");
    fs::remove_file(path).unwrap();
  }
}

#[test]
fn reads_back_what_it_writes() {
  let path = file(
    "write", "OLD=1
",
  );
  let peer = Peer {
    url: "https://host:8788".into(),
    token: "abc".into(),
  };
  peer::write(&path, &peer).unwrap();
  assert_eq!(peer::read(&path).unwrap(), Some(peer.clone()));
  let broken = Peer {
    token: "a
b"
    .into(),
    ..peer
  };
  assert!(peer::write(&path, &broken).is_err());
  fs::remove_file(path).unwrap();
}
