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

#[test]
fn parses_a_node_list() {
  use peer::{Node, Place};
  let nodes = peer::parse_nodes(
    "# name  where\n\ndo      https://203.0.113.5:8788  # the droplet\n\
     pi      push\n",
  )
  .unwrap();
  assert_eq!(
    nodes,
    [
      Node {
        name: "do".into(),
        place: Place::Url("https://203.0.113.5:8788".into()),
      },
      Node {
        name: "pi".into(),
        place: Place::Push,
      },
    ]
  );
  for (bad, why) in [
    ("do\n", "not `<name>"),
    ("do https://a extra\n", "not `<name>"),
    ("d.o https://a\n", "a name is"),
    ("do ftp://a\n", "not a URL"),
    ("do https://a\ndo https://b\n", "line 2: listed twice"),
  ] {
    let error = peer::parse_nodes(bad).unwrap_err();
    assert!(error.contains(why), "{bad:?}: {error}");
  }
}

#[test]
fn names_peers_by_host() {
  assert_eq!(peer::name_of("http://old-host:8788"), "old-host");
  assert_eq!(peer::name_of("https://203.0.113.5:8788/x"), "203-0-113-5");
}

/// A fresh install root, removed when dropped.
struct Root(PathBuf);

impl Root {
  fn new(name: &str) -> Self {
    let path = ::std::env::temp_dir()
      .join(format!("deploy-peers-{name}-{}", ::std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(path.join("peers")).unwrap();
    Self(path)
  }

  fn write(&self, path: &str, text: &str) {
    fs::write(self.0.join(path), text).unwrap();
  }
}

impl Drop for Root {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

#[test]
fn gathers_peers_from_every_source() {
  use peer::{Named, Source};
  let root = Root::new("all");
  assert_eq!(peer::gather(&root.0), peer::Peers::default());

  root.write(
    "nodes",
    "do https://do:8788\noracle https://oracle:8788\npi push\n",
  );
  // A node list without mesh.env isn't used.
  let peers = peer::gather(&root.0);
  assert!(peers.pull.is_empty());
  assert!(peers.problems[0].contains("not used without"), "{peers:?}");

  root.write("mesh.env", "LOGGER_NODE=pi\nLOGGER_PEER_TOKEN=mesh\n");
  root.write(
    "peers/old.env",
    "LOGGER_PEER_URL=http://old:8788\nLOGGER_PEER_TOKEN=one\n",
  );
  // The same URL twice is synced from once.
  root.write(
    "peers/again.env",
    "LOGGER_PEER_URL=https://do:8788\nLOGGER_PEER_TOKEN=two\n",
  );
  root.write(
    "peer.env",
    "LOGGER_PEER_URL=http://legacy-host:8788\nLOGGER_PEER_TOKEN=three\n",
  );
  root.write("peers/broken.env", "LOGGER_PEER_URL=http://x\n");
  let peers = peer::gather(&root.0);
  assert_eq!(peers.node.as_deref(), Some("pi"));
  let named = |name: &str, url: &str, token: &str, source| Named {
    name: name.into(),
    peer: Peer {
      url: url.into(),
      token: token.into(),
    },
    source,
  };
  assert_eq!(
    peers.pull,
    [
      named("do", "https://do:8788", "mesh", Source::Nodes),
      named("oracle", "https://oracle:8788", "mesh", Source::Nodes),
      named("old", "http://old:8788", "one", Source::File),
      named(
        "legacy-host",
        "http://legacy-host:8788",
        "three",
        Source::File
      ),
    ]
  );
  assert_eq!(peers.problems.len(), 1, "{:?}", peers.problems);
  assert!(peers.problems[0].contains("broken.env"));

  // A node not in the list still syncs from every node in it, and says so.
  root.write("mesh.env", "LOGGER_NODE=new\nLOGGER_PEER_TOKEN=mesh\n");
  let peers = peer::gather(&root.0);
  assert!(peers.pull.iter().any(|named| named.name == "do"));
  assert!(
    peers
      .problems
      .iter()
      .any(|p| p.contains("new, isn't listed"))
  );
}

#[test]
fn writes_the_mesh_file() {
  let root = Root::new("mesh");
  let path = root.0.join("mesh.env");
  let mesh = peer::Mesh {
    node: "do".into(),
    token: "abc".into(),
  };
  peer::write_mesh(&path, &mesh).unwrap();
  assert_eq!(peer::read_mesh(&path).unwrap(), Some(mesh.clone()));
  let renamed = peer::Mesh {
    node: "a b".into(),
    ..mesh
  };
  assert!(peer::write_mesh(&path, &renamed).is_err());
}
