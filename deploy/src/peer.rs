//! The peers an install syncs from, read by the supervisor before each
//! round, so editing them takes effect without reinstalling. Tokens never
//! go on a command line: `sync` gets its token in its environment.
//!
//! They're found in the install's root:
//!
//! - `nodes`: the mesh's node list, the same file on every node (see
//!   [`parse_nodes`]). Every other node with a URL is a peer.
//! - `mesh.env`: which node this is ([`NODE`]) and the mesh's read token
//!   ([`TOKEN`]). The node list is used only with it.
//! - `peers\<name>.env`: one-off peers ([`URL`], [`TOKEN`]), such as a host
//!   being moved off.
//! - `peer.env`: the single peer of installs from before there were
//!   several, named for its URL's host.
//!
//! The `.env` files hold `KEY=VALUE` lines; blank lines and `#` comments
//! are skipped, and a value may be quoted.

use ::std::{
  collections::HashMap,
  fs, io,
  path::{Path, PathBuf},
};

/// A peer's `serve`, e.g. `https://203.0.113.5:8788`.
pub const URL: &str = "LOGGER_PEER_URL";
/// Its read token, as `dgg-logger sync` reads it from the environment.
pub const TOKEN: &str = "LOGGER_PEER_TOKEN";
/// This node's name in the node list.
pub const NODE: &str = "LOGGER_NODE";

/// The node list, in an install's root.
pub const NODES: &str = "nodes";
/// This node's name and the mesh token, in an install's root.
pub const MESH: &str = "mesh.env";
/// The directory of one-off peers, in an install's root.
pub const PEERS: &str = "peers";
/// The single peer of older installs, in an install's root.
pub const LEGACY: &str = "peer.env";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
  pub url: String,
  pub token: String,
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
  /// This node's name, from `mesh.env`.
  pub node: Option<String>,
  /// The peers to sync from, each URL once: the node list's first, then the
  /// files', by name.
  pub pull: Vec<Named>,
  /// What couldn't be read, each skipped.
  pub problems: Vec<String>,
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

/// The `KEY=VALUE` pairs in `path`; `None` if there's no file.
fn read_env(path: &Path) -> io::Result<Option<HashMap<String, String>>> {
  let text = match fs::read_to_string(path) {
    Ok(text) => text,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(e),
  };
  let mut pairs = HashMap::new();
  for line in text.lines().map(str::trim) {
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let Some((key, value)) = line.split_once('=') else {
      return Err(invalid(path, &format!("not KEY=VALUE: {line:?}")));
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

/// Writes `pairs` to `path` as `KEY=VALUE` lines, replacing what's there.
/// An existing file keeps its permissions.
fn write_env(path: &Path, pairs: &[(&str, &str)]) -> io::Result<()> {
  let mut text = String::new();
  for (key, value) in pairs {
    if value.is_empty() || value.contains(['\n', '\r']) {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{key} must be one non-empty line"),
      ));
    }
    text += &format!("{key}={value}\n");
  }
  fs::write(path, text)
}

/// Writes `peer` to `path`, replacing what's there.
pub fn write(path: &Path, peer: &Peer) -> io::Result<()> {
  write_env(path, &[(URL, &peer.url), (TOKEN, &peer.token)])
}

/// The peer in `path`; `None` if there's no file.
pub fn read(path: &Path) -> io::Result<Option<Peer>> {
  let Some(pairs) = read_env(path)? else {
    return Ok(None);
  };
  let [url, token] = values(path, &pairs, [URL, TOKEN])?;
  Ok(Some(Peer { url, token }))
}

/// This node's name and the mesh token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mesh {
  pub node: String,
  pub token: String,
}

/// Writes `mesh` to `path`, replacing what's there.
pub fn write_mesh(path: &Path, mesh: &Mesh) -> io::Result<()> {
  if !valid_name(&mesh.node) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "a node's name is letters, digits, - and _",
    ));
  }
  write_env(path, &[(NODE, &mesh.node), (TOKEN, &mesh.token)])
}

/// The mesh in `path`; `None` if there's no file.
pub fn read_mesh(path: &Path) -> io::Result<Option<Mesh>> {
  let Some(pairs) = read_env(path)? else {
    return Ok(None);
  };
  let [node, token] = values(path, &pairs, [NODE, TOKEN])?;
  Ok(Some(Mesh { node, token }))
}

/// The one-off peer files in `root`'s `peers` directory, by name.
pub fn files(root: &Path) -> io::Result<Vec<(String, PathBuf)>> {
  let entries = match fs::read_dir(root.join(PEERS)) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(e),
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

/// The path of the one-off peer `name` under `root`.
pub fn file(root: &Path, name: &str) -> io::Result<PathBuf> {
  if !valid_name(name) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "a peer's name is letters, digits, - and _",
    ));
  }
  Ok(root.join(PEERS).join(format!("{name}.env")))
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

/// The peers found in `root`. A file that can't be read is a problem, and
/// the rest are still found.
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
  let mesh = match read_mesh(&root.join(MESH)) {
    Ok(mesh) => mesh,
    Err(e) => {
      peers.problems.push(e.to_string());
      None
    }
  };
  let nodes_path = root.join(NODES);
  let nodes = match fs::read_to_string(&nodes_path) {
    Ok(text) => match parse_nodes(&text) {
      Ok(nodes) => Some(nodes),
      Err(e) => {
        peers
          .problems
          .push(format!("{}: {e}", nodes_path.display()));
        None
      }
    },
    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
    Err(e) => {
      peers
        .problems
        .push(format!("{}: {e}", nodes_path.display()));
      None
    }
  };
  match (&mesh, nodes) {
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
    (None, Some(_)) => peers.problems.push(format!(
      "{}: not used without {}",
      nodes_path.display(),
      root.join(MESH).display()
    )),
    (None, None) => {}
  }
  let mut files = match files(root) {
    Ok(files) => files,
    Err(e) => {
      peers
        .problems
        .push(format!("{}: {e}", root.join(PEERS).display()));
      Vec::new()
    }
  };
  files.push((String::new(), root.join(LEGACY)));
  for (name, path) in files {
    match read(&path) {
      Ok(Some(peer)) => {
        let name = if name.is_empty() {
          name_of(&peer.url)
        } else {
          name
        };
        add(
          &mut peers,
          Named {
            name,
            peer,
            source: Source::File,
          },
        );
      }
      Ok(None) => {}
      Err(e) => peers.problems.push(e.to_string()),
    }
  }
  peers
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
