use {
  ::common::config::Secret,
  ::deploy::peer::{self, Change, Config, Mesh, Peer, Serve, ServeTokens},
  ::std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
  },
};

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

  fn config(&self) -> Config {
    peer::read_config(&self.0).unwrap().unwrap()
  }
}

impl Drop for Root {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn peer(url: &str, token: &str) -> Peer {
  Peer {
    url: url.into(),
    token: Secret::new(token),
  }
}

/// No securing needed.
fn open(_: &Path) -> ::std::io::Result<()> {
  Ok(())
}

#[test]
fn gathers_peers_from_the_mesh_and_the_config() {
  use peer::{Named, Source};
  let root = Root::new("all");
  assert_eq!(peer::gather(&root.0), peer::Peers::default());

  root.write(
    "nodes",
    "do https://do:8788\noracle https://oracle:8788\npi push\n",
  );
  // A node list without a mesh isn't used.
  let peers = peer::gather(&root.0);
  assert!(peers.pull.is_empty());
  assert!(peers.problems[0].contains("not used without"), "{peers:?}");

  root.write(
    "install.env.json",
    r#"{
      "ca": "the CA",
      "mesh": {"node": "pi", "token": "mesh"},
      "peers": {
        "old": {"url": "http://old:8788", "token": "one"},
        "again": {"url": "https://do:8788", "token": "two"}
      }
    }"#,
  );
  let peers = peer::gather(&root.0);
  assert_eq!(peers.node.as_deref(), Some("pi"));
  assert_eq!(peers.ca.as_deref(), Some("the CA"));
  let named = |name: &str, url: &str, token: &str, source| Named {
    name: name.into(),
    peer: peer(url, token),
    source,
  };
  // The same URL twice is synced from once.
  assert_eq!(
    peers.pull,
    [
      named("do", "https://do:8788", "mesh", Source::Nodes),
      named("oracle", "https://oracle:8788", "mesh", Source::Nodes),
      named("old", "http://old:8788", "one", Source::File),
    ]
  );
  assert_eq!(peers.problems, Vec::<String>::new());
  assert!(!peers.unreadable);

  // A node not in the list still syncs from every node in it, and says so.
  root.write(
    "install.env.json",
    r#"{"mesh": {"node": "new", "token": "mesh"}}"#,
  );
  let peers = peer::gather(&root.0);
  assert!(peers.pull.iter().any(|named| named.name == "do"));
  assert!(
    peers
      .problems
      .iter()
      .any(|p| p.contains("new, isn't listed"))
  );

  // Files from before the config are said, never read.
  root.write("mesh.env", "LOGGER_NODE=pi\nLOGGER_PEER_TOKEN=old\n");
  root.write("peers/x.env", "anything");
  let problems = peer::gather(&root.0).problems;
  assert_eq!(problems.len(), 3, "{problems:?}");
  assert!(problems[1].contains("mesh.env"), "{problems:?}");
  assert!(problems[2].contains("x.env"), "{problems:?}");
}

#[test]
fn a_bad_config_is_said_without_its_secrets() {
  let root = Root::new("bad");
  for (text, why) in [
    (
      r#"{"peers": {"a": {"url": "http://a", "token": "sekrit"}}"#,
      "EOF",
    ),
    (
      r#"{"peers": {"a": {"url": "http://a", "tokne": "sekrit"}}}"#,
      "unknown field `tokne`",
    ),
    (
      r#"{"peers": {"a": {"url": "http://a"}}}"#,
      "missing field `token`",
    ),
    (
      r#"{"mesh": {"node": "me", "token": ["sekrit"]}}"#,
      "the wrong type",
    ),
    (
      r#"{"peers": {"a": {"url": "ftp://a", "token": "sekrit"}}}"#,
      "peer a: not a URL",
    ),
    (
      r#"{"mesh": {"node": "a b", "token": "sekrit"}}"#,
      "a node's name",
    ),
  ] {
    root.write("install.env.json", text);
    let error = peer::read_config(&root.0).unwrap_err();
    assert!(error.contains(why), "{text}: {error}");
    assert!(!error.contains("sekrit"), "{error}");
    let found = peer::gather(&root.0);
    assert!(found.unreadable);
    assert_eq!(found.problems, [error]);
  }
}

#[test]
fn applies_changes() {
  let root = Root::new("apply");
  let secured = RefCell::new(Vec::new());
  let secure = |path: &Path| {
    // Secured while still empty: before the secrets go in.
    assert_eq!(fs::read(path).unwrap(), b"");
    secured.borrow_mut().push(path.to_owned());
    Ok(())
  };
  let apply = |changes: &[Change]| peer::apply(&root.0, changes, &secure);
  apply(&[
    Change::Peer {
      name: "old".into(),
      peer: peer("http://old:8788", "one"),
    },
    Change::Nodes("do https://do:8788\nme push\n".into()),
    Change::Mesh(Mesh {
      node: "me".into(),
      token: Secret::new("mesh"),
    }),
  ])
  .unwrap();
  // Written once, to a file renamed into place.
  assert_eq!(*secured.borrow(), [root.0.join("install.env.json.tmp")]);
  let names = |root: &Root| -> Vec<String> {
    peer::gather(&root.0)
      .pull
      .into_iter()
      .map(|named| named.name)
      .collect()
  };
  assert_eq!(names(&root), ["do", "old"]);
  assert_eq!(
    peer::secrets(&root.0).unwrap(),
    [root.0.join("install.env.json")]
  );
  // The secrets are in the file.
  let text = fs::read_to_string(root.0.join("install.env.json")).unwrap();
  assert!(text.contains(r#""token": "one""#), "{text}");
  apply(&[Change::Ca("the CA".into())]).unwrap();
  assert_eq!(root.config().ca.as_deref(), Some("the CA"));
  // Nothing changed: nothing written.
  apply(&[Change::NoPeer("missing".into())]).unwrap();
  assert_eq!(secured.borrow().len(), 2);

  // A bad node list isn't written.
  assert!(apply(&[Change::Nodes("do ftp://x\n".into())]).is_err());
  assert_eq!(names(&root), ["do", "old"]);
  apply(&[Change::NoPeer("old".into())]).unwrap();
  assert_eq!(names(&root), ["do"]);
  apply(&[Change::NoMesh]).unwrap();
  assert!(names(&root).is_empty());
  assert!(!root.0.join("nodes").exists());
  assert!(apply(&[Change::NoPeer("../x".into())]).is_err());
  // A config that can't be read is left alone.
  root.write("install.env.json", "{");
  assert!(apply(&[Change::NoPeers]).is_err());
  assert_eq!(
    fs::read_to_string(root.0.join("install.env.json")).unwrap(),
    "{"
  );
}

#[test]
fn folds_an_older_installs_files_in() {
  let root = Root::new("migrate");
  // Nothing to fold: nothing written.
  assert!(peer::migrate(&root.0, &open).unwrap().is_empty());
  assert!(!root.0.join("install.env.json").exists());

  root.write("mesh.env", "LOGGER_NODE=me\nLOGGER_PEER_TOKEN=mesh\n");
  root.write(
    "peers/a.env",
    "LOGGER_PEER_URL=http://a:8788\nLOGGER_PEER_TOKEN='one'\n",
  );
  root.write(
    "peer.env",
    "LOGGER_PEER_URL=http://old-host:8788\nLOGGER_PEER_TOKEN=two\n",
  );
  root.write("ca.pem", "the CA\n");
  let secured = RefCell::new(0);
  let said = peer::migrate(&root.0, &|_| {
    *secured.borrow_mut() += 1;
    Ok(())
  })
  .unwrap();
  assert_eq!(said, Vec::<String>::new());
  assert_eq!(*secured.borrow(), 1);
  let config = root.config();
  assert_eq!(
    config,
    Config {
      ca: Some("the CA\n".into()),
      mesh: Some(Mesh {
        node: "me".into(),
        token: Secret::new("mesh"),
      }),
      peers: [
        ("a".into(), peer("http://a:8788", "one")),
        ("old-host".into(), peer("http://old-host:8788", "two")),
      ]
      .into(),
      serve: None,
    }
  );
  for old in ["mesh.env", "peers", "peer.env", "ca.pem"] {
    assert!(!root.0.join(old).exists(), "{old}");
  }
  assert!(peer::gather(&root.0).problems.is_empty());

  // What the config has already is kept; what can't be read is left.
  fs::create_dir(root.0.join("peers")).unwrap();
  root.write(
    "peers/a.env",
    "LOGGER_PEER_URL=http://other:8788\nLOGGER_PEER_TOKEN=three\n",
  );
  root.write("peers/b.env", "LOGGER_PEER_URL=http://b:8788\n");
  let said = peer::migrate(&root.0, &open).unwrap();
  assert_eq!(said.len(), 2, "{said:?}");
  assert!(said[0].contains("peer a was set already"), "{said:?}");
  assert!(said[1].contains("b.env: needs"), "{said:?}");
  assert!(!said.iter().any(|said| said.contains("three")));
  assert_eq!(root.config(), config);
  assert!(!root.0.join("peers/a.env").exists());
  assert!(root.0.join("peers/b.env").exists());
}

#[test]
fn moves_the_config_between_roots() {
  let (old, new) = (Root::new("move-old"), Root::new("move-new"));
  old.write("nodes", "do https://do:8788\n");
  old.write(
    "install.env.json",
    r#"{"ca": "the CA", "mesh": {"node": "me", "token": "mesh"},
        "peers": {"a": {"url": "http://a:8788", "token": "old"},
                  "b": {"url": "http://b:8788", "token": "old"}}}"#,
  );
  // An older install's files come along too, for the next migration.
  old.write("peers/c.env", "LOGGER_PEER_URL=http://c\n");
  // The new root's own peer of a name is kept.
  new.write(
    "install.env.json",
    r#"{"peers": {"b": {"url": "http://b:8788", "token": "new"}}}"#,
  );
  let said = peer::move_all(&old.0, &new.0).unwrap();
  assert_eq!(said.len(), 1, "{said:?}");
  let found = peer::gather(&new.0);
  let tokens: Vec<(&str, &str)> = found
    .pull
    .iter()
    .map(|named| (named.name.as_str(), named.peer.token.expose()))
    .collect();
  assert_eq!(tokens, [("do", "mesh"), ("a", "old"), ("b", "new")]);
  assert_eq!(found.ca.as_deref(), Some("the CA"));
  assert!(new.0.join("peers/c.env").exists());
  assert!(peer::gather(&old.0).pull.is_empty());
  assert!(!old.0.join("peers").exists());
  assert!(!old.0.join("install.env.json").exists());
  // Into a root without a config, it's copied as it is.
  let newer = Root::new("move-newer");
  fs::remove_dir(newer.0.join("peers")).unwrap();
  assert!(peer::move_all(&new.0, &newer.0).unwrap().is_empty());
  assert_eq!(newer.config().peers.len(), 2);
}

/// Made-up tokens, long enough to serve with.
const MESH: &str = "a-made-up-mesh-token-for-tests-x";
const QUERY: &str = "a-made-up-query-token-for-tests-";

/// A certificate and its key, from a new CA.
fn node_cert() -> (String, String) {
  let key = ::rcgen::KeyPair::generate().unwrap();
  let ca = ::logger::cert::ca(&key).unwrap();
  let node = ::logger::cert::node("me", &["127.0.0.1".into()], &ca, &key);
  let node = node.unwrap();
  (node.cert, node.key.to_string())
}

fn serve(cert: Option<&str>, key: Option<&str>) -> Serve {
  Serve {
    listen: "0.0.0.0:8788".into(),
    cert: cert.map(Into::into),
    key: key.map(Secret::new),
    tokens: ServeTokens {
      query: Some(Secret::new(QUERY)),
      ..ServeTokens::default()
    },
  }
}

#[test]
fn serves_with_the_mesh_token_unless_given_one() {
  let root = Root::new("serve");
  let (cert, key) = node_cert();
  let mesh = Mesh {
    node: "me".into(),
    token: Secret::new(MESH),
  };
  peer::apply(
    &root.0,
    &[
      Change::Mesh(mesh.clone()),
      Change::Serve(serve(Some(&cert), Some(&key))),
    ],
    &open,
  )
  .unwrap();
  let config = root.config();
  let serving = config.serve.as_ref().unwrap();
  assert_eq!(*serving, serve(Some(&cert), Some(&key)));
  // What serve reads on its stdin: the mesh's token, its own query token,
  // and the certificate and key.
  let given = serving.config(config.mesh.as_ref()).unwrap();
  let given: ::serde_json::Value =
    ::serde_json::from_str(given.expose()).unwrap();
  assert_eq!(given["tokens"]["read"], MESH);
  assert_eq!(given["tokens"]["query"], QUERY);
  assert_eq!(given["tokens"]["control"], ::serde_json::Value::Null);
  assert_eq!(given["tls"]["cert"], cert.as_str());
  assert_eq!(given["tls"]["key"], key.as_str());
  // Shown, the key and tokens are secrets; the certificate isn't.
  let shown = ::serde_json::to_string(&config).unwrap();
  assert!(!shown.contains(MESH) && !shown.contains(QUERY), "{shown}");
  assert!(!shown.contains("PRIVATE KEY"), "{shown}");
  assert!(shown.contains("BEGIN CERTIFICATE"), "{shown}");

  // Without a mesh or a read token of its own, it isn't run, and that's
  // said.
  peer::apply(&root.0, &[Change::NoMesh], &open).unwrap();
  let config = root.config();
  assert!(config.serve.as_ref().unwrap().config(None).is_none());
  let problems = peer::gather(&root.0).problems;
  assert!(
    problems[0].contains("serve has no read token"),
    "{problems:?}"
  );
  peer::apply(&root.0, &[Change::NoServe], &open).unwrap();
  assert_eq!(root.config().serve, None);
  assert!(peer::gather(&root.0).problems.is_empty());
}

#[test]
fn refuses_to_serve_with_what_wont_do() {
  let (cert, key) = node_cert();
  let mesh = Mesh {
    node: "me".into(),
    token: Secret::new(MESH),
  };
  let (_, other_key) = node_cert();
  let short = |serve: Serve| Serve {
    tokens: ServeTokens {
      control: Some(Secret::new("short")),
      ..serve.tokens
    },
    ..serve
  };
  let same = |serve: Serve| Serve {
    tokens: ServeTokens {
      query: Some(Secret::new(MESH)),
      ..serve.tokens
    },
    ..serve
  };
  for (serve, why) in [
    (
      Serve {
        listen: "everywhere".into(),
        ..serve(None, None)
      },
      "listen isn't an address",
    ),
    (serve(Some(&cert), None), "a cert needs its key"),
    (serve(Some(&cert), Some(&other_key)), "serve: "),
    (short(serve(None, None)), "the control token must be"),
    (same(serve(None, None)), "must differ"),
  ] {
    let error = serve.check(Some(&mesh)).unwrap_err();
    assert!(error.contains(why), "{why}: {error}");
    assert!(!error.contains(MESH), "{error}");
  }
  assert_eq!(serve(Some(&cert), Some(&key)).check(Some(&mesh)), Ok(()));
}
