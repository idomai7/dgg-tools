//! The peers an install syncs from, and the rest of its config, read by
//! the supervisor before each round, so editing them takes effect without
//! reinstalling. Secrets never go on a command line or in an environment:
//! `sync` gets its token in its config, on stdin.
//!
//! They're found in the install's root:
//!
//! - `install.env.json` ([`CONFIG`]): the install's config, secrets
//!   included ([`Config`]): the mesh CA's certificate, this node's name and
//!   the mesh's read token, one-off peers, such as a host being moved off,
//!   and what `serve` serves with, if the supervisor runs it.
//!   `dgg-logger config` shows it without its secrets.
//! - `nodes`: the mesh's node list, the same file on every node (see
//!   [`parse_nodes`]). With the config's `mesh`, every other node with a URL
//!   is a peer.
//!
//! Installs from before kept the secrets in `mesh.env`, `peers\<name>.env`
//! and `peer.env`, and the CA in `ca.pem` ([`OLD`]): [`migrate`] folds them
//! into the config, and until then they're warned of, never read.

use {
  ::common::{
    atomic_file::{replace_with, temporary},
    config::{Secret, Source as Input, read},
    fs::MissingOk,
  },
  ::serde::{Deserialize, Serialize},
  ::serde_json::{Map, Value, json},
  ::std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
  },
  ::zeroize::Zeroizing,
};

/// The install's config, in an install's root.
pub const CONFIG: &str = "install.env.json";
/// The node list, in an install's root.
pub const NODES: &str = "nodes";

/// This node's name and the mesh token, before [`CONFIG`].
const MESH: &str = "mesh.env";
/// The directory of one-off peers, before [`CONFIG`].
const PEERS: &str = "peers";
/// The single peer of older installs still.
const LEGACY: &str = "peer.env";
/// The mesh CA's certificate, before [`CONFIG`].
const CA: &str = "ca.pem";
/// What installs from before [`CONFIG`] kept in their root.
pub const OLD: [&str; 4] = [MESH, PEERS, LEGACY, CA];

/// The keys of the old `.env` files.
const URL: &str = "LOGGER_PEER_URL";
const TOKEN: &str = "LOGGER_PEER_TOKEN";
const NODE: &str = "LOGGER_NODE";

/// An install's config, as [`CONFIG`] holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  /// The mesh CA's certificate (PEM): with it, every `https://` peer is
  /// reached trusting that CA alone.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub ca: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mesh: Option<Mesh>,
  /// One-off peers, by name.
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub peers: BTreeMap<String, Peer>,
  /// With it, the supervisor runs `serve`.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub serve: Option<Serve>,
}

/// What the supervisor runs `serve` with.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Serve {
  /// Where to listen, e.g. `0.0.0.0:8788`.
  pub listen: String,
  /// This node's certificate (PEM), from `cert node`: with `key`, it
  /// serves HTTPS.
  #[serde(default)]
  pub cert: Option<String>,
  /// The certificate's key (PEM).
  #[serde(default)]
  pub key: Option<Secret>,
  #[serde(default)]
  pub tokens: ServeTokens,
}

/// `serve`'s bearer tokens (see `dgg-logger serve`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServeTokens {
  /// Reads the journal; the mesh's token if left out, which the other
  /// nodes sync with.
  #[serde(default)]
  pub read: Option<Secret>,
  /// Lets another host's capture take over from this one's.
  #[serde(default)]
  pub control: Option<Secret>,
  /// Reads only Kick's history and live chat, for kick-mod.
  #[serde(default)]
  pub query: Option<Secret>,
}

impl Serve {
  /// Its read token: its own, else the mesh's.
  fn read<'a>(&'a self, mesh: Option<&'a Mesh>) -> Option<&'a Secret> {
    self.tokens.read.as_ref().or(mesh.map(|mesh| &mesh.token))
  }

  /// Why it can't be served with, if it can't, its read token the mesh's
  /// if it has none of its own; with neither, the rest is checked.
  pub fn check(&self, mesh: Option<&Mesh>) -> Result<(), String> {
    self.listen.parse::<::std::net::SocketAddr>().map_err(|_| {
      format!("serve: listen isn't an address: {}", self.listen)
    })?;
    match (&self.cert, &self.key) {
      (Some(cert), Some(key)) => {
        let key = Zeroizing::new(key.expose().to_owned());
        ::logger::cert::server_config(cert, &key)
          .map_err(|e| format!("serve: {e}"))?;
      }
      (None, None) => {}
      _ => {
        return Err("serve: a cert needs its key, and a key its cert".into());
      }
    }
    let expose = |token: &Option<Secret>| {
      token.as_ref().map(|token| token.expose().to_owned())
    };
    let tokens = ::logger::serve::Tokens {
      // Placeholder enough to check the others.
      read: match self.read(mesh) {
        Some(read) => read.expose().to_owned(),
        None => "-".repeat(::logger::serve::MIN_TOKEN),
      },
      control: expose(&self.tokens.control),
      query: expose(&self.tokens.query),
    };
    tokens.check().map_err(|why| format!("serve: {why}"))
  }

  /// What `serve` reads with `--config-stdin`: its tokens, the read token
  /// the mesh's if it has none of its own, and its certificate and key.
  /// `None` with neither.
  pub fn config(&self, mesh: Option<&Mesh>) -> Option<Secret> {
    let expose = |token: &Option<Secret>| {
      token.as_ref().map(|token| token.expose().to_owned())
    };
    let mut config = json!({
      "tokens": {
        "read": self.read(mesh)?.expose(),
        "control": expose(&self.tokens.control),
        "query": expose(&self.tokens.query),
      }
    });
    if let (Some(cert), Some(key)) = (&self.cert, &self.key) {
      config["tls"] = json!({ "cert": cert, "key": key.expose() });
    }
    Some(Secret::new(config.to_string()))
  }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
  /// Its `serve`, e.g. `https://203.0.113.5:8788`.
  pub url: String,
  /// Its read token.
  pub token: Secret,
}

/// This node's name and the mesh token.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mesh {
  pub node: String,
  pub token: Secret,
}

impl Config {
  /// Why it can't be used, if it can't.
  fn check(&self) -> Result<(), String> {
    if let Some(mesh) = &self.mesh {
      if !valid_name(&mesh.node) {
        return Err("a node's name is letters, digits, - and _".into());
      }
      if mesh.token.expose().is_empty() {
        return Err("the mesh's token is empty".into());
      }
    }
    if let Some(serve) = &self.serve {
      serve.check(self.mesh.as_ref())?;
    }
    for (name, peer) in &self.peers {
      if !valid_name(name) {
        return Err("a peer's name is letters, digits, - and _".into());
      }
      if !valid_url(&peer.url) {
        return Err(format!("peer {name}: not a URL (http:// or https://)"));
      }
      if peer.token.expose().is_empty() {
        return Err(format!("peer {name}: its token is empty"));
      }
    }
    Ok(())
  }

  /// As written to [`CONFIG`]: its secrets too.
  fn exposed(&self) -> Zeroizing<String> {
    let mut config = Map::new();
    if let Some(ca) = &self.ca {
      config.insert("ca".into(), ca.clone().into());
    }
    if let Some(mesh) = &self.mesh {
      let mesh = json!({ "node": mesh.node, "token": mesh.token.expose() });
      config.insert("mesh".into(), mesh);
    }
    if !self.peers.is_empty() {
      let peers = self.peers.iter().map(|(name, peer)| {
        let peer = json!({ "url": peer.url, "token": peer.token.expose() });
        (name.clone(), peer)
      });
      config.insert("peers".into(), Value::Object(peers.collect()));
    }
    if let Some(serve) = &self.serve {
      let expose = |secret: &Option<Secret>| {
        secret.as_ref().map(|secret| secret.expose().to_owned())
      };
      let serve = json!({
        "listen": serve.listen,
        "cert": serve.cert,
        "key": expose(&serve.key),
        "tokens": {
          "read": expose(&serve.tokens.read),
          "control": expose(&serve.tokens.control),
          "query": expose(&serve.tokens.query),
        },
      });
      config.insert("serve".into(), serve);
    }
    let text = ::serde_json::to_string_pretty(&Value::Object(config));
    Zeroizing::new(text.unwrap() + "\n")
  }

  /// `old`'s parts that this lacks, saying of each it has already that
  /// `old`'s (from `whence`) was dropped.
  fn fold(&mut self, old: Config, whence: &Path, said: &mut Vec<String>) {
    let mut kept = |what: &str| {
      said.push(format!(
        "{}: {what} was set already, and kept",
        whence.display()
      ));
    };
    match (&self.ca, old.ca) {
      (None, ca) => self.ca = ca,
      (Some(_), Some(_)) => kept("the CA"),
      (Some(_), None) => {}
    }
    match (&self.mesh, old.mesh) {
      (None, mesh) => self.mesh = mesh,
      (Some(_), Some(_)) => kept("the mesh"),
      (Some(_), None) => {}
    }
    match (&self.serve, old.serve) {
      (None, serve) => self.serve = serve,
      (Some(_), Some(_)) => kept("serve"),
      (Some(_), None) => {}
    }
    for (name, peer) in old.peers {
      match self.peers.contains_key(&name) {
        true => kept(&format!("peer {name}")),
        false => _ = self.peers.insert(name, peer),
      }
    }
  }
}

/// Where a node in the node list is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
  /// Reachable at this URL.
  Url(String),
  /// Behind NAT: it pushes its own captures instead.
  Push,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
  pub name: String,
  pub place: Place,
}

/// Where a peer came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
  Nodes,
  /// The config's one-off peers.
  File,
}

/// A peer to sync from, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
  pub name: String,
  pub peer: Peer,
  pub source: Source,
}

/// Everything found in an install's root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peers {
  /// This node's name, from the config's `mesh`.
  pub node: Option<String>,
  /// The peers to sync from, each URL once: the node list's first, then the
  /// config's, by name.
  pub pull: Vec<Named>,
  /// The mesh CA's certificate (PEM), if there is one.
  pub ca: Option<String>,
  /// What couldn't be read, each skipped.
  pub problems: Vec<String>,
  /// Whether the config itself couldn't be read.
  pub unreadable: bool,
}

/// A name: letters, digits, `-` and `_`.
fn valid_name(name: &str) -> bool {
  !name.is_empty()
    && name
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_url(url: &str) -> bool {
  url.starts_with("http://") || url.starts_with("https://")
}

/// A node list: one `<name> <URL>` or `<name> push` per line; blank lines
/// and `#` comments are skipped.
pub fn parse_nodes(text: &str) -> Result<Vec<Node>, String> {
  let mut nodes: Vec<Node> = Vec::new();
  for (number, line) in text.lines().enumerate() {
    let line = line.split('#').next().unwrap_or_default().trim();
    if line.is_empty() {
      continue;
    }
    let bad = |why: &str| format!("line {}: {why}: {line:?}", number + 1);
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [name, place] = fields[..] else {
      return Err(bad("not `<name> <URL>` or `<name> push`"));
    };
    if !valid_name(name) {
      return Err(bad("a name is letters, digits, - and _"));
    }
    if nodes.iter().any(|node| node.name == name) {
      return Err(bad("listed twice"));
    }
    let place = match place {
      "push" => Place::Push,
      url if valid_url(url) => Place::Url(url.to_owned()),
      _ => return Err(bad("not a URL (http:// or https://) or push")),
    };
    nodes.push(Node {
      name: name.to_owned(),
      place,
    });
  }
  Ok(nodes)
}

/// The config in `root`; `None` if there's none. Its errors never quote a
/// secret.
pub fn read_config(root: &Path) -> Result<Option<Config>, String> {
  let path = root.join(CONFIG);
  if !path.exists() {
    return Ok(None);
  }
  let config: Config = read(Input::File(&path))?;
  config
    .check()
    .map_err(|why| format!("{}: {why}", path.display()))?;
  Ok(Some(config))
}

/// Writes `config` to `root`, replacing what's there whole. The new file is
/// given to `secure` while still empty, before its secrets go in.
fn write_config(
  root: &Path,
  config: &Config,
  secure: &dyn Fn(&Path) -> io::Result<()>,
) -> io::Result<()> {
  config.check().map_err(invalid_input)?;
  let path = root.join(CONFIG);
  replace_with(&path, |file| {
    secure(&temporary(&path))?;
    file.write_all(config.exposed().as_bytes())
  })
}

/// A change `install` makes to an install's config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
  /// Sync from `peer` as `name`, replacing any of that name.
  Peer { name: String, peer: Peer },
  /// Stop syncing from the one-off peer `name`, if there is one.
  NoPeer(String),
  /// Stop syncing from every one-off peer.
  NoPeers,
  /// Use this node list (checked by [`parse_nodes`]).
  Nodes(String),
  /// Be this node, with this mesh token.
  Mesh(Mesh),
  /// Leave the mesh: the node list and the config's mesh go.
  NoMesh,
  /// Trust this CA certificate (PEM) alone for `https://` peers.
  Ca(String),
  /// Serve with this, replacing what it served with.
  Serve(Serve),
  /// Stop serving.
  NoServe,
}

/// Makes `changes` under `root`, in order, the config written once (see
/// [`write_config`] for `secure`). A config that can't be read is left as
/// it is, and nothing changed.
pub fn apply(
  root: &Path,
  changes: &[Change],
  secure: &dyn Fn(&Path) -> io::Result<()>,
) -> io::Result<()> {
  let mut config = read_config(root).map_err(invalid_data)?;
  let before = config.clone();
  for change in changes {
    match change {
      Change::Peer { name, peer } => {
        if !valid_name(name) {
          return Err(invalid_input(
            "a peer's name is letters, digits, - and _",
          ));
        }
        config
          .get_or_insert_with(Config::default)
          .peers
          .insert(name.clone(), peer.clone());
      }
      Change::NoPeer(name) => {
        if !valid_name(name) {
          return Err(invalid_input(
            "a peer's name is letters, digits, - and _",
          ));
        }
        if let Some(config) = &mut config {
          config.peers.remove(name);
        }
      }
      Change::NoPeers => {
        if let Some(config) = &mut config {
          config.peers.clear();
        }
      }
      Change::Nodes(text) => {
        parse_nodes(text).map_err(invalid_input)?;
        fs::create_dir_all(root)?;
        // Whole, as the supervisor may read it any time.
        replace_with(&root.join(NODES), |file| {
          file.write_all(text.as_bytes())
        })?;
      }
      Change::Mesh(mesh) => {
        config.get_or_insert_with(Config::default).mesh = Some(mesh.clone())
      }
      Change::NoMesh => {
        remove(&root.join(NODES))?;
        if let Some(config) = &mut config {
          config.mesh = None;
        }
      }
      Change::Ca(text) => {
        config.get_or_insert_with(Config::default).ca = Some(text.clone())
      }
      Change::Serve(serve) => {
        config.get_or_insert_with(Config::default).serve = Some(serve.clone())
      }
      Change::NoServe => {
        if let Some(config) = &mut config {
          config.serve = None;
        }
      }
    }
  }
  match config {
    Some(config) if Some(&config) != before.as_ref() => {
      write_config(root, &config, secure)
    }
    _ => Ok(()),
  }
}

/// The files under `root` holding secrets: the config, and any from before
/// it.
pub fn secrets(root: &Path) -> io::Result<Vec<PathBuf>> {
  let mut secrets: Vec<PathBuf> = old_peer_files(root)?
    .into_iter()
    .map(|(_, path)| path)
    .collect();
  secrets.extend([root.join(CONFIG), root.join(MESH), root.join(LEGACY)]);
  secrets.retain(|path| path.exists());
  Ok(secrets)
}

/// Folds the files of an install from before [`CONFIG`] into it (see
/// [`write_config`] for `secure`), then removes them. What the config has
/// already is kept; what's dropped, or can't be read (and is left as it
/// is), is said.
pub fn migrate(
  root: &Path,
  secure: &dyn Fn(&Path) -> io::Result<()>,
) -> io::Result<Vec<String>> {
  let mut config = read_config(root).map_err(invalid_data)?;
  let mut said = Vec::new();
  let mut folded = Vec::new();
  let mut fold = |old: io::Result<Option<Config>>, path: PathBuf| match old {
    Ok(Some(old)) => {
      let config = config.get_or_insert_with(Config::default);
      config.fold(old, &path, &mut said);
      folded.push(path);
    }
    Ok(None) => {}
    Err(e) => said.push(format!("{e}; left as it is")),
  };
  let mesh = root.join(MESH);
  fold(read_mesh(&mesh).map(|mesh| mesh.map(Config::of_mesh)), mesh);
  let mut files = old_peer_files(root)?;
  files.push((String::new(), root.join(LEGACY)));
  for (name, path) in files {
    let peer = read_peer(&path).map(|peer| {
      peer.map(|peer| {
        let name = match name.is_empty() {
          true => name_of(&peer.url),
          false => name,
        };
        Config::of_peer(name, peer)
      })
    });
    fold(peer, path);
  }
  let ca = root.join(CA);
  let pem = fs::read_to_string(&ca).missing_ok().map(|pem| {
    pem.map(|pem| Config {
      ca: Some(pem),
      ..Config::default()
    })
  });
  fold(pem, ca);
  let Some(config) = config.filter(|_| !folded.is_empty()) else {
    return Ok(said);
  };
  write_config(root, &config, secure)?;
  for path in folded {
    remove(&path)?;
  }
  // Only if it's empty now.
  let _ = fs::remove_dir(root.join(PEERS));
  Ok(said)
}

impl Config {
  fn of_mesh(mesh: Mesh) -> Self {
    Self {
      mesh: Some(mesh),
      ..Self::default()
    }
  }

  fn of_peer(name: String, peer: Peer) -> Self {
    Self {
      peers: BTreeMap::from([(name, peer)]),
      ..Self::default()
    }
  }
}

/// Brings `old`'s config and node list along to `new`, then removes them
/// from `old`. What `new` has already is kept: a config's parts are folded
/// in (see [`migrate`]), and said if dropped. Copied, they take `new`'s
/// permissions.
pub fn move_all(old: &Path, new: &Path) -> io::Result<Vec<String>> {
  let mut said = Vec::new();
  let mut paths: Vec<PathBuf> =
    vec![NODES.into(), MESH.into(), LEGACY.into(), CA.into()];
  for (name, _) in old_peer_files(old)? {
    paths.push(Path::new(PEERS).join(format!("{name}.env")));
  }
  for path in &paths {
    let (from, to) = (old.join(path), new.join(path));
    if from.exists() && !to.exists() {
      fs::create_dir_all(to.parent().unwrap())?;
      fs::copy(&from, &to)?;
    }
  }
  let theirs = read_config(old).map_err(invalid_data)?;
  match (read_config(new).map_err(invalid_data)?, theirs) {
    (None, Some(_)) => {
      fs::create_dir_all(new)?;
      fs::copy(old.join(CONFIG), new.join(CONFIG))?;
    }
    (Some(mut config), Some(theirs)) => {
      config.fold(theirs, &old.join(CONFIG), &mut said);
      write_config(new, &config, &|_: &Path| Ok(()))?;
    }
    (_, None) => {}
  }
  remove_all(old)?;
  Ok(said)
}

/// Removes the config and node list under `root`, and any files from
/// before the config: nothing's left to use their tokens.
pub fn remove_all(root: &Path) -> io::Result<()> {
  for path in [NODES, CONFIG, MESH, LEGACY, CA] {
    remove(&root.join(path))?;
  }
  fs::remove_dir_all(root.join(PEERS)).missing_ok()?;
  Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
  fs::remove_file(path).missing_ok()?;
  Ok(())
}

/// Where to look for peers, given `--peers` or the `--peer-file` of older
/// installs (whose directory is the install's root).
pub fn root(
  peers: Option<PathBuf>,
  peer_file: Option<PathBuf>,
) -> Option<PathBuf> {
  peers.or_else(|| Some(peer_file?.parent()?.to_owned()))
}

/// A name for a peer known only by its URL: its host.
pub fn name_of(url: &str) -> String {
  let host = url
    .split_once("://")
    .map_or(url, |(_, rest)| rest)
    .split(['/', ':'])
    .next()
    .unwrap_or_default();
  let name: String = host
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
    .collect();
  if name.is_empty() { "peer".into() } else { name }
}

/// The peers found in `root`. A config or node list that can't be read is
/// a problem, and the rest is still found; so is a file from before the
/// config, which isn't read.
pub fn gather(root: &Path) -> Peers {
  let mut peers = Peers::default();
  let add = |peers: &mut Peers, named: Named| {
    if !peers
      .pull
      .iter()
      .any(|known| known.peer.url == named.peer.url)
    {
      peers.pull.push(named);
    }
  };
  let config = match read_config(root) {
    Ok(config) => config.unwrap_or_default(),
    Err(e) => {
      peers.problems.push(e);
      peers.unreadable = true;
      Config::default()
    }
  };
  let nodes_path = root.join(NODES);
  let nodes = match fs::read_to_string(&nodes_path).missing_ok() {
    Ok(None) => None,
    Ok(Some(text)) => match parse_nodes(&text) {
      Ok(nodes) => Some(nodes),
      Err(e) => {
        peers
          .problems
          .push(format!("{}: {e}", nodes_path.display()));
        None
      }
    },
    Err(e) => {
      peers
        .problems
        .push(format!("{}: {e}", nodes_path.display()));
      None
    }
  };
  match (&config.mesh, nodes) {
    (Some(mesh), Some(nodes)) => {
      peers.node = Some(mesh.node.clone());
      if !nodes.iter().any(|node| node.name == mesh.node) {
        peers.problems.push(format!(
          "{}: this node, {}, isn't listed",
          nodes_path.display(),
          mesh.node
        ));
      }
      for node in nodes.into_iter().filter(|node| node.name != mesh.node) {
        if let Place::Url(url) = node.place {
          let peer = Peer {
            url,
            token: mesh.token.clone(),
          };
          add(
            &mut peers,
            Named {
              name: node.name,
              peer,
              source: Source::Nodes,
            },
          );
        }
      }
    }
    (Some(mesh), None) => peers.node = Some(mesh.node.clone()),
    // Not for want of one, if the config couldn't be read.
    (None, Some(_)) if !peers.unreadable => peers.problems.push(format!(
      "{}: not used without a mesh in {}",
      nodes_path.display(),
      root.join(CONFIG).display()
    )),
    (None, _) => {}
  }
  for (name, peer) in config.peers {
    add(
      &mut peers,
      Named {
        name,
        peer,
        source: Source::File,
      },
    );
  }
  peers.ca = config.ca;
  if let Some(serve) = &config.serve
    && serve.read(config.mesh.as_ref()).is_none()
  {
    peers.problems.push(format!(
      "{}: serve has no read token, and there's no mesh to take one from; \
       it isn't run",
      root.join(CONFIG).display()
    ));
  }
  for path in old_files(root) {
    peers.problems.push(format!(
      "{}: from before {CONFIG}, and not read; installing again folds it in",
      path.display()
    ));
  }
  peers
}

/// The files of an install from before [`CONFIG`] under `root`, by name.
pub fn old_files(root: &Path) -> Vec<PathBuf> {
  let mut old: Vec<PathBuf> = [MESH, LEGACY, CA]
    .into_iter()
    .map(|name| root.join(name))
    .filter(|path| path.exists())
    .collect();
  old.extend(
    old_peer_files(root)
      .unwrap_or_default()
      .into_iter()
      .map(|(_, path)| path),
  );
  old
}

/// The one-off peer files from before [`CONFIG`] in `root`'s `peers`
/// directory, by name.
fn old_peer_files(root: &Path) -> io::Result<Vec<(String, PathBuf)>> {
  let Some(entries) = fs::read_dir(root.join(PEERS)).missing_ok()? else {
    return Ok(Vec::new());
  };
  let mut files = Vec::new();
  for entry in entries {
    let path = entry?.path();
    if path.extension().is_some_and(|extension| extension == "env")
      && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
    {
      files.push((name.to_owned(), path));
    }
  }
  files.sort();
  Ok(files)
}

/// The `KEY=VALUE` pairs in an old `.env` file at `path`; `None` if there's
/// no file. Blank lines and `#` comments are skipped, and a value may be
/// quoted.
fn read_env(path: &Path) -> io::Result<Option<HashMap<String, String>>> {
  let Some(text) = fs::read_to_string(path).missing_ok()? else {
    return Ok(None);
  };
  let mut pairs = HashMap::new();
  for line in text.lines().map(str::trim) {
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let Some((key, value)) = line.split_once('=') else {
      return Err(invalid(path, "not KEY=VALUE lines"));
    };
    pairs.insert(key.trim().to_owned(), unquote(value.trim()).to_owned());
  }
  Ok(Some(pairs))
}

/// The non-empty values of `keys` in `path`'s pairs.
fn values<const N: usize>(
  path: &Path,
  pairs: &HashMap<String, String>,
  keys: [&str; N],
) -> io::Result<[String; N]> {
  let mut found = Vec::new();
  for key in keys {
    match pairs.get(key) {
      Some(value) if !value.is_empty() => found.push(value.clone()),
      _ => return Err(invalid(path, &format!("needs {}", keys.join(" and ")))),
    }
  }
  Ok(found.try_into().unwrap())
}

/// The peer in an old `.env` file at `path`; `None` if there's no file.
fn read_peer(path: &Path) -> io::Result<Option<Peer>> {
  let Some(pairs) = read_env(path)? else {
    return Ok(None);
  };
  let [url, token] = values(path, &pairs, [URL, TOKEN])?;
  if !valid_url(&url) {
    return Err(invalid(path, "not a URL (http:// or https://)"));
  }
  let token = Secret::new(token);
  Ok(Some(Peer { url, token }))
}

/// The mesh in an old `mesh.env` at `path`; `None` if there's no file.
fn read_mesh(path: &Path) -> io::Result<Option<Mesh>> {
  let Some(pairs) = read_env(path)? else {
    return Ok(None);
  };
  let [node, token] = values(path, &pairs, [NODE, TOKEN])?;
  if !valid_name(&node) {
    return Err(invalid(path, "a node's name is letters, digits, - and _"));
  }
  let token = Secret::new(token);
  Ok(Some(Mesh { node, token }))
}

fn unquote(value: &str) -> &str {
  for quote in ['"', '\''] {
    if let Some(inner) = value
      .strip_prefix(quote)
      .and_then(|value| value.strip_suffix(quote))
    {
      return inner;
    }
  }
  value
}

fn invalid(path: &Path, what: &str) -> io::Error {
  io::Error::new(
    io::ErrorKind::InvalidData,
    format!("{}: {what}", path.display()),
  )
}

fn invalid_data(why: String) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, why)
}

fn invalid_input(why: impl Into<String>) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidInput, why.into())
}
