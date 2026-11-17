//! Make the certificates a mesh's nodes serve TLS with.

use {
  crate::cert::{self, CA_DAYS, NODE_DAYS},
  ::clap::{Args, Subcommand},
  ::rcgen::KeyPair,
  ::std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
  },
  ::zeroize::Zeroizing,
};

/// Make the certificates a mesh's nodes serve TLS with: the mesh's own CA,
/// and each node's certificate, signed by it. Run it on your own machine:
/// the CA's key never goes to a node.
#[derive(Args)]
pub struct Cert {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Make the CA's certificate (for 10 years) from the key at --ca-key; if
  /// there's none, a new ECDSA P-256 key is written there first, protected
  /// with a password you choose. Run it again to renew the certificate:
  /// nodes' certificates signed before still verify.
  Ca {
    #[command(flatten)]
    files: CaFiles,
    /// Write a new key without a password.
    #[arg(long)]
    no_password: bool,
  },
  /// Sign a certificate (for 2 years) for a node, with a new key; run it
  /// again to renew one.
  Node {
    /// The node's name, as in the node list.
    name: String,
    /// Where other nodes reach it, as in its URL: an IP address or DNS
    /// name. Repeat for several.
    #[arg(long = "host", required = true)]
    hosts: Vec<String>,
    #[command(flatten)]
    files: CaFiles,
    /// The folder to write `<name>.pem` and `<name>.key` to [default: a
    /// `nodes` folder beside the CA's certificate].
    #[arg(long)]
    out: Option<PathBuf>,
  },
}

#[derive(Args)]
struct CaFiles {
  /// The CA's private key: PKCS#8 PEM, with a password or not (ECDSA P-256
  /// or P-384, Ed25519 or RSA).
  #[arg(long, env = "DGG_CA_KEY", default_value = "infra/secrets/ca.key")]
  ca_key: PathBuf,
  /// The CA's certificate.
  #[arg(long, env = "DGG_CA_CERT", default_value = "infra/secrets/ca.pem")]
  ca_cert: PathBuf,
}

impl Cert {
  pub fn run(self) -> ExitCode {
    let done = match self.command {
      Command::Ca { files, no_password } => make_ca(&files, no_password),
      Command::Node {
        name,
        hosts,
        files,
        out,
      } => make_node(&name, &hosts, &files, out),
    };
    match done {
      Ok(()) => ExitCode::SUCCESS,
      Err(e) => {
        eprintln!("{e}");
        ExitCode::FAILURE
      }
    }
  }
}

/// Asks for a password, without echoing it.
fn ask(prompt: &str) -> io::Result<Zeroizing<String>> {
  ::rpassword::prompt_password(prompt).map(Zeroizing::new)
}

/// The key at `path`, asking for its password if it has one.
fn read_key(path: &Path) -> Result<KeyPair, String> {
  let pem = Zeroizing::new(
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
  );
  cert::read_key(&pem, || ask(&format!("password for {}: ", path.display())))
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes `contents` to `path`, readable only by its owner where that can
/// be set (elsewhere, as its folder allows), creating its folder; with
/// `new`, only if there's nothing there yet.
fn write_secret(path: &Path, contents: &str, new: bool) -> io::Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }
  let mut options = OpenOptions::new();
  match new {
    true => options.write(true).create_new(true),
    false => options.write(true).create(true).truncate(true),
  };
  #[cfg(unix)]
  ::std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
  let mut file = options.open(path)?;
  #[cfg(unix)]
  {
    use ::std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
  }
  file.write_all(contents.as_bytes())?;
  file.sync_all()
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
  let write = || {
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
  };
  write().map_err(|e| format!("{}: {e}", path.display()))
}

fn make_ca(files: &CaFiles, no_password: bool) -> Result<(), String> {
  let path = &files.ca_key;
  let key = match path.exists() {
    true => {
      let key = read_key(path)?;
      eprintln!("using the key in {}", path.display());
      key
    }
    false => {
      let password = match no_password {
        true => None,
        false => {
          let ask = |prompt: &str| {
            ask(prompt).map_err(|e| format!("can't read the password: {e}"))
          };
          let password =
            ask(&format!("new password for {}: ", path.display()))?;
          if password.is_empty() {
            return Err(
              "no password given (--no-password writes the key without \
               one)"
                .into(),
            );
          }
          if ask("again: ")? != password {
            return Err("the passwords differ".into());
          }
          Some(password)
        }
      };
      let key =
        KeyPair::generate().map_err(|e| format!("can't make a key: {e}"))?;
      let pem = cert::key_pem(&key, password.as_deref().map(String::as_str))?;
      write_secret(path, &pem, true)
        .map_err(|e| format!("{}: {e}", path.display()))?;
      eprintln!(
        "wrote a new key to {}{}; keep it safe, off the nodes",
        path.display(),
        match password {
          Some(_) => ", protected by your password",
          None => ", without a password",
        }
      );
      key
    }
  };
  write(&files.ca_cert, &cert::ca(&key)?)?;
  eprintln!(
    "wrote the CA certificate to {} (for {CA_DAYS} days)",
    files.ca_cert.display()
  );
  Ok(())
}

fn make_node(
  name: &str,
  hosts: &[String],
  files: &CaFiles,
  out: Option<PathBuf>,
) -> Result<(), String> {
  let ca_cert = fs::read_to_string(&files.ca_cert)
    .map_err(|e| format!("{}: {e}", files.ca_cert.display()))?;
  let ca_key = read_key(&files.ca_key)?;
  let node = cert::node(name, hosts, &ca_cert, &ca_key)?;
  drop(ca_key);
  let out = out.unwrap_or_else(|| {
    files
      .ca_cert
      .parent()
      .unwrap_or(Path::new("."))
      .join("nodes")
  });
  let cert = out.join(format!("{name}.pem"));
  let key = out.join(format!("{name}.key"));
  write(&cert, &node.cert)?;
  write_secret(&key, &node.key, false)
    .map_err(|e| format!("{}: {e}", key.display()))?;
  eprintln!(
    "wrote {name}'s certificate (for {NODE_DAYS} days, for {}) to {}, and \
     its key to {}",
    hosts.join(", "),
    cert.display(),
    key.display()
  );
  Ok(())
}
