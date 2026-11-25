//! Run, install and inspect the logger as a long-lived service.

use {
  ::clap::{Parser, Subcommand},
  ::deploy::{
    install,
    paths::{Paths, user_root},
    status::{self, Watcher},
    supervise::{self, Config, Exit, Stop},
  },
  ::logger::cli,
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
};

#[derive(Parser)]
#[command(name = "dgg-logger", version = ::deploy::BUILD)]
struct Args {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Keep capture and `index --follow` running against a data directory,
  /// compacting every hour and (with `--peers`) syncing every 15
  /// minutes, until stopped (Ctrl-C) or until capture hands off to a newer
  /// instance. With `--peers`, `kick-capture` runs too, once the
  /// `kick-channels.jsonc` there has channels.
  Supervise {
    /// Data directory: the journal and the index.
    #[arg(long)]
    data: PathBuf,
    /// Where the logs go.
    #[arg(long)]
    logs: PathBuf,
    /// Where `dgg-logger` is, to run its commands (default:
    /// beside this program).
    #[arg(long)]
    bin_dir: Option<PathBuf>,
    /// Minutes between compactions.
    #[arg(long, default_value_t = 60)]
    compact_every: u64,
    /// Where the peers to sync from are: an install's root, holding its
    /// config (`install.env.json`) and node list (`nodes`; see the
    /// README), looked at before each round.
    #[arg(long)]
    peers: Option<PathBuf>,
    /// A single peer file, as older installs ran: its directory is taken as
    /// `--peers`.
    #[arg(long, hide = true, conflicts_with = "peers")]
    peer_file: Option<PathBuf>,
    /// Minutes between syncs.
    #[arg(long, default_value_t = 15)]
    sync_every: u64,
  },
  /// Install the logger from the build this program is in: for this user,
  /// as a scheduled task started at logon, or with `--machine` as a
  /// Windows service started with the system. Installing again upgrades;
  /// installing the other kind switches to it, moving the data. Nothing is
  /// missed on the way: a capture from the new build bridges the change.
  #[cfg(windows)]
  Install {
    /// A machine install (a service): needs an elevated prompt.
    #[arg(long)]
    machine: bool,
    /// Where the install goes (default: the installed one's, else
    /// `%LOCALAPPDATA%\dgg-tools`, or `C:\ProgramData\dgg-tools` with
    /// `--machine`).
    #[arg(long)]
    root: Option<PathBuf>,
    /// The data directory, if not `<root>\data`.
    #[arg(long)]
    data: Option<PathBuf>,
    /// The logs directory, if not `<root>\logs`.
    #[arg(long)]
    logs: Option<PathBuf>,
    /// Builds to keep in `<root>\bin`.
    #[arg(long, default_value_t = 3)]
    keep: usize,
    /// Be this node of a mesh, syncing from every other node in
    /// `--nodes` with the mesh's token, `mesh_token` in the config given.
    /// Kept in `<root>\install.env.json` until changed or removed.
    #[arg(long)]
    name: Option<String>,
    /// The mesh's node list (see the README), copied to `<root>\nodes`.
    #[arg(long)]
    nodes: Option<PathBuf>,
    /// Leave the mesh: its node list and the config's mesh are removed.
    #[arg(long, conflicts_with_all = ["name", "nodes"])]
    no_mesh: bool,
    /// The mesh CA's certificate (from `cert ca`), kept in the config:
    /// `https://` peers are then trusted on its word alone.
    #[arg(long)]
    ca: Option<PathBuf>,
    /// Also sync from this host's `serve` every 15 minutes (e.g.
    /// `https://old-host:8788`), with its token, `peer_token` in the
    /// config given. Kept in the config until changed or removed.
    #[arg(long)]
    peer: Option<String>,
    /// The name for `--peer` (default: its host).
    #[arg(long, requires = "peer")]
    peer_name: Option<String>,
    /// Serve this install's journal for other hosts and kick-mod, listening
    /// here (e.g. `0.0.0.0:8788`), with the tokens in the config given's
    /// `serve`; its read token is the mesh's if it has none. Kept in the
    /// config, replacing what was served with, until changed or removed.
    #[arg(long)]
    serve: Option<String>,
    /// This node's certificate for `--serve`, from `cert node`, kept in the
    /// config: it serves HTTPS with it.
    #[arg(long, requires_all = ["serve", "serve_key"])]
    serve_cert: Option<PathBuf>,
    /// The certificate's key, kept in the config.
    #[arg(long, requires = "serve_cert")]
    serve_key: Option<PathBuf>,
    /// Stop serving.
    #[arg(long, conflicts_with = "serve")]
    no_serve: bool,
    /// The secrets for `--name`, `--peer` and `--serve`: `{"mesh_token":
    /// "...", "peer_token": "...", "serve": {"read": "...", "control":
    /// "...", "query": "..."}}`, each only with its flag, and `serve`'s
    /// tokens each optional.
    #[command(flatten)]
    secrets: cli::ConfigFrom,
    /// Stop syncing from the peer of this name.
    #[arg(long)]
    no_peer: Vec<String>,
    /// Stop syncing from every peer set with `--peer`.
    #[arg(long)]
    no_peers: bool,
  },
  /// Remove the install: its task or service, programs and binaries. The
  /// data and logs stay (a machine install's get ordinary permissions,
  /// owned by you). A machine install needs an elevated prompt.
  #[cfg(windows)]
  Uninstall {
    /// Where to clean up if nothing is installed (after shell runs).
    #[arg(long)]
    root: Option<PathBuf>,
  },
  /// Show the install's config, `install.env.json`, with every secret as
  /// `<secret>`, and anything wrong with it.
  Config {
    /// The install's root (default: the installed one, else the user
    /// install's).
    #[arg(long)]
    root: Option<PathBuf>,
  },
  /// Show what the logger is doing: its programs, each capture's health,
  /// the journal, the index, free space and the ends of the logs.
  Status {
    /// The install's root, holding `bin`, `data` and `logs` (default: the
    /// installed one, else the user install's).
    #[arg(long)]
    root: Option<PathBuf>,
    /// The data directory, if not `<root>/data`.
    #[arg(long)]
    data: Option<PathBuf>,
    /// The logs directory, if not `<root>/logs`.
    #[arg(long)]
    logs: Option<PathBuf>,
    /// Stay open, redrawing every `--interval` seconds, with a log below.
    #[arg(long)]
    watch: bool,
    #[arg(long, default_value_t = 1.0)]
    interval: f64,
    /// Lines of each log to show, without `--watch`.
    #[arg(long, default_value_t = 5)]
    lines: usize,
  },
  DggCapture(WithData<cli::Capture>),
  Index(WithData<cli::Index>),
  Compact(WithData<cli::Compact>),
  Migrate(WithData<cli::Migrate>),
  Serve(WithData<cli::Serve>),
  Sync(WithData<cli::Sync>),
  DggBackfill(WithData<cli::Backfill>),
  DggGaps(WithData<cli::Gaps>),
  Cert(cli::Cert),
  KickIds(WithRoot<cli::KickIds>),
  KickCapture(WithData<cli::KickCapture>),
}

// One of the logger's own commands, with the data directory it works on
// (not a doc comment, which would replace the command's own).
#[derive(::clap::Args)]
struct WithData<T: ::clap::Args> {
  /// Data directory, holding the journal and the index (default: the
  /// installed logger's).
  #[arg(long)]
  data: Option<PathBuf>,
  #[command(flatten)]
  command: T,
}

impl<T: ::clap::Args> WithData<T> {
  /// `run` on the data directory.
  fn run(
    self,
    run: impl FnOnce(T, &::std::path::Path) -> ExitCode,
  ) -> ExitCode {
    let data = self.data.unwrap_or_else(|| installed().data);
    run(self.command, &data)
  }
}

// One of the logger's own commands, with the install it works on.
#[derive(::clap::Args)]
struct WithRoot<T: ::clap::Args> {
  /// The install's root (default: the installed logger's, else the user
  /// install's).
  #[arg(long)]
  root: Option<PathBuf>,
  #[command(flatten)]
  command: T,
}

impl<T: ::clap::Args> WithRoot<T> {
  /// `run` on the install's root.
  fn run(
    self,
    run: impl FnOnce(T, &::std::path::Path) -> ExitCode,
  ) -> ExitCode {
    let root = self.root.unwrap_or_else(|| installed().root);
    run(self.command, &root)
  }
}

/// What `install --config-file` or `--config-stdin` gives.
#[cfg(windows)]
#[derive(::serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallSecrets {
  /// The mesh's read token, for `--name`.
  mesh_token: Option<::common::config::Secret>,
  /// The token for `--peer`.
  peer_token: Option<::common::config::Secret>,
  /// The tokens for `--serve`.
  serve: Option<::deploy::peer::ServeTokens>,
}

/// Prints the install's config without its secrets, then anything wrong
/// with it, as `status` sees it.
fn show_config(root: Option<PathBuf>) -> ExitCode {
  use ::deploy::peer::{CONFIG, gather, read_config};
  let root = root.unwrap_or_else(|| installed().root);
  let failed = match read_config(&root) {
    Ok(Some(config)) => {
      // Every secret shows as <secret>.
      println!("{}", ::serde_json::to_string_pretty(&config).unwrap());
      false
    }
    Ok(None) => {
      println!("no {CONFIG} in {}", root.display());
      false
    }
    Err(_) => true,
  };
  for problem in gather(&root).problems {
    eprintln!("{problem}");
  }
  match failed {
    true => ExitCode::FAILURE,
    false => ExitCode::SUCCESS,
  }
}

fn main() -> ExitCode {
  match Args::parse().command {
    Command::Supervise {
      data,
      logs,
      bin_dir,
      compact_every,
      peers,
      peer_file,
      sync_every,
    } => supervise(
      data,
      logs,
      bin_dir,
      Schedule {
        compact_every,
        peers: ::deploy::peer::root(peers, peer_file),
        sync_every,
      },
    ),
    #[cfg(windows)]
    Command::Install {
      machine,
      root,
      data,
      logs,
      keep,
      name,
      nodes,
      no_mesh,
      ca,
      peer,
      peer_name,
      serve,
      serve_cert,
      serve_key,
      no_serve,
      secrets,
      no_peer,
      no_peers,
    } => {
      use ::deploy::peer::{self, Change, Mesh, Peer};
      let secrets = match secrets.read::<InstallSecrets>() {
        Ok(secrets) => secrets.unwrap_or(InstallSecrets {
          mesh_token: None,
          peer_token: None,
          serve: None,
        }),
        Err(e) => {
          eprintln!("{e}");
          return ExitCode::FAILURE;
        }
      };
      let needs = |flag: &str, key: &str| {
        eprintln!(
          "{flag} needs {key} in a config given with --config-file or \
           --config-stdin: {{\"{key}\": \"...\"}}"
        );
        ExitCode::FAILURE
      };
      let unused = |key: &str, flag: &str| {
        eprintln!("the config's {key} is only for {flag}");
        ExitCode::FAILURE
      };
      let mut peers = Vec::new();
      if no_mesh {
        peers.push(Change::NoMesh);
      }
      if let Some(nodes) = nodes {
        let text = match ::std::fs::read_to_string(&nodes) {
          Ok(text) => text,
          Err(e) => {
            eprintln!("{}: {e}", nodes.display());
            return ExitCode::FAILURE;
          }
        };
        if let Err(e) = peer::parse_nodes(&text) {
          eprintln!("{}: {e}", nodes.display());
          return ExitCode::FAILURE;
        }
        peers.push(Change::Nodes(text));
      }
      if let Some(ca) = ca {
        let text = match ::std::fs::read_to_string(&ca) {
          Ok(text) => text,
          Err(e) => {
            eprintln!("{}: {e}", ca.display());
            return ExitCode::FAILURE;
          }
        };
        if let Err(e) = ::logger::cert::roots(&text) {
          eprintln!("{}: {e}", ca.display());
          return ExitCode::FAILURE;
        }
        peers.push(Change::Ca(text));
      }
      match (name, secrets.mesh_token) {
        (Some(node), Some(token)) => {
          peers.push(Change::Mesh(Mesh { node, token }))
        }
        (Some(_), None) => return needs("--name", "mesh_token"),
        (None, Some(_)) => return unused("mesh_token", "--name"),
        (None, None) => {}
      }
      if no_peers {
        peers.push(Change::NoPeers);
      }
      peers.extend(no_peer.into_iter().map(Change::NoPeer));
      match (peer, secrets.peer_token) {
        (Some(url), _)
          if !url.starts_with("http://") && !url.starts_with("https://") =>
        {
          eprintln!("--peer is a URL: https://host:port");
          return ExitCode::FAILURE;
        }
        (Some(url), Some(token)) => peers.push(Change::Peer {
          name: peer_name.unwrap_or_else(|| peer::name_of(&url)),
          peer: Peer { url, token },
        }),
        (Some(_), None) => return needs("--peer", "peer_token"),
        (None, Some(_)) => return unused("peer_token", "--peer"),
        (None, None) => {}
      }
      if no_serve {
        peers.push(Change::NoServe);
      }
      match (serve, secrets.serve) {
        (Some(listen), tokens) => {
          let read = |path: Option<&PathBuf>| {
            path
              .map(|path| {
                ::std::fs::read_to_string(path)
                  .map_err(|e| format!("{}: {e}", path.display()))
              })
              .transpose()
          };
          let (cert, key) =
            match (read(serve_cert.as_ref()), read(serve_key.as_ref())) {
              (Ok(cert), Ok(key)) => (cert, key),
              (Err(e), _) | (_, Err(e)) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
              }
            };
          let serve = peer::Serve {
            listen,
            cert,
            key: key.map(::common::config::Secret::new),
            tokens: tokens.unwrap_or_default(),
          };
          // Without the mesh, which may be in the config already.
          if let Err(e) = serve.check(None) {
            eprintln!("{e}");
            return ExitCode::FAILURE;
          }
          peers.push(Change::Serve(serve));
        }
        (None, Some(_)) => return unused("serve", "--serve"),
        (None, None) => {}
      }
      install(&install::Options {
        scope: if machine {
          install::Scope::Machine
        } else {
          install::Scope::User
        },
        root,
        data,
        logs,
        keep: keep.max(1),
        peers,
        ..install::Options::default()
      })
    }
    #[cfg(windows)]
    Command::Uninstall { root } => uninstall(root),
    Command::Config { root } => show_config(root),
    Command::Status {
      root,
      data,
      logs,
      watch,
      interval,
      lines,
    } => {
      let mut paths = match root {
        Some(root) => Paths::under(root),
        None => installed(),
      };
      paths.data = data.unwrap_or(paths.data);
      paths.logs = logs.unwrap_or(paths.logs);
      let result = if watch {
        status::watch::run(&paths, Duration::from_secs_f64(interval))
      } else {
        print_status(&paths, lines)
      };
      match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
          eprintln!("status failed: {e}");
          ExitCode::FAILURE
        }
      }
    }
    Command::DggCapture(command) => command.run(cli::Capture::run),
    Command::Index(command) => command.run(cli::Index::run),
    Command::Compact(command) => command.run(cli::Compact::run),
    Command::Migrate(command) => command.run(cli::Migrate::run),
    Command::Serve(command) => command.run(cli::Serve::run),
    Command::Sync(command) => command.run(cli::Sync::run),
    Command::DggBackfill(command) => command.run(cli::Backfill::run),
    Command::DggGaps(command) => command.run(cli::Gaps::run),
    Command::Cert(command) => command.run(),
    Command::KickCapture(command) => command.run(cli::KickCapture::run),
    Command::KickIds(command) => command.run(|command, root| {
      command.run(&root.join(::logger::kick::channels::FILE))
    }),
  }
}

/// The installed logger's directories, else a user install's.
fn installed() -> Paths {
  match install::installed() {
    Ok(Some(installed)) => installed.paths(),
    _ => Paths::under(user_root()),
  }
}

#[cfg(windows)]
fn install(options: &install::Options) -> ExitCode {
  eprintln!("installing; waiting for the new capture to be healthy");
  match install::install(options) {
    Ok(done) => {
      let kind = match done.scope {
        install::Scope::User => "user install (a logon task)",
        install::Scope::Machine => "machine install (a service)",
      };
      match &done.replaced {
        None => eprintln!("installed: {kind}"),
        Some(replaced) if replaced.scope() == done.scope => {
          eprintln!("upgraded: {kind}")
        }
        Some(_) => eprintln!("switched to a {kind}"),
      }
      eprintln!("  build    {}", done.build.display());
      eprintln!("  data     {}", done.paths.data.display());
      eprintln!("  logs     {}", done.paths.logs.display());
      for line in &done.lists {
        eprintln!("  lists    {line}");
      }
      for (name, capture) in &done.dgg {
        match capture {
          Some(capture) => eprintln!("  dgg      {name}: {capture} (healthy)"),
          None => eprintln!("  dgg      {name}: not healthy yet"),
        }
      }
      for (slug, capture) in &done.kick {
        match capture {
          Some(capture) => eprintln!("  kick     {slug}: {capture} (healthy)"),
          None => eprintln!("  kick     {slug}: not healthy yet"),
        }
      }
      for peer in &done.peers {
        eprintln!("  syncs    from {peer} every 15 minutes");
      }
      for problem in &done.problems {
        eprintln!("  warning  {problem}");
      }
      eprintln!("`dgg-logger status --watch` to see it");
      for build in done.left {
        eprintln!("still in use, left for next time: {}", build.display());
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("install failed: {e}");
      ExitCode::FAILURE
    }
  }
}

#[cfg(windows)]
fn uninstall(root: Option<PathBuf>) -> ExitCode {
  match install::uninstall(root) {
    Ok(removed) => {
      match removed.paths {
        Some(paths) => eprintln!(
          "removed the task and the binaries in {}",
          paths.bin().display()
        ),
        None => eprintln!("nothing installed"),
      }
      for path in removed.kept {
        eprintln!("kept {}", path.display());
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("uninstall failed: {e}");
      ExitCode::FAILURE
    }
  }
}

fn print_status(paths: &Paths, lines: usize) -> ::std::io::Result<()> {
  let report = Watcher::new()?.gather(paths)?;
  for line in status::text(&report) {
    println!("{line}");
  }
  for log in status::logs(&report) {
    let path = paths.logs.join(format!("{}.log", log.name));
    if let Ok(tail) = status::tail(&path, lines) {
      println!();
      println!("LOG {}", path.display());
      for line in tail {
        println!("  {line}");
      }
    }
  }
  Ok(())
}

/// What `supervise` runs on a schedule, in minutes.
struct Schedule {
  compact_every: u64,
  peers: Option<PathBuf>,
  sync_every: u64,
}

fn supervise(
  data: PathBuf,
  logs: PathBuf,
  bin_dir: Option<PathBuf>,
  schedule: Schedule,
) -> ExitCode {
  let bin_dir = match bin_dir {
    Some(bin_dir) => bin_dir,
    None => match beside_this_program() {
      Some(bin_dir) => bin_dir,
      None => {
        eprintln!("can't tell where this program is; pass --bin-dir");
        return ExitCode::FAILURE;
      }
    },
  };
  let mut config = Config::new(&bin_dir, &data, &logs);
  config.compact_every = Duration::from_secs(schedule.compact_every * 60);
  config.peers = schedule.peers;
  config.dgg_channels = supervise::dgg_channels(config.peers.as_deref());
  config.kick_channels = supervise::kick_channels(config.peers.as_deref());
  config.sync_every = Duration::from_secs(schedule.sync_every * 60);
  let stop = Stop::new();
  let on_signal = stop.clone();
  if let Err(e) = ::ctrlc::set_handler(move || on_signal.request()) {
    eprintln!("can't handle Ctrl-C: {e}");
    return ExitCode::FAILURE;
  }
  eprintln!("supervising; logs in {}", logs.display());
  match supervise::run(&config, &stop) {
    Ok(Exit::Stopped) => ExitCode::SUCCESS,
    Ok(Exit::HandedOff) => {
      eprintln!("capture handed off to a newer instance");
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("supervise failed: {e}");
      ExitCode::FAILURE
    }
  }
}

fn beside_this_program() -> Option<PathBuf> {
  Some(::std::env::current_exe().ok()?.parent()?.to_owned())
}
