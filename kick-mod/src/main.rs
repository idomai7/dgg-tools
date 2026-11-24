//! A chat client and moderation tools for Kick channels.

use {
  ::clap::{Parser, Subcommand},
  ::common::config::Source,
  ::kick_api::official::{Api, api},
  ::kick_mod::{
    account::{self, Account},
    channels::{self, Channels},
    chat::Chat,
    config::{self, Config},
    logger, modlog,
    serve::{self, Served},
  },
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
  },
  ::tokio::net::TcpListener,
};

#[derive(Parser)]
#[command(name = "kick-mod")]
struct Args {
  /// The settings and secrets, as JSON.
  #[arg(
    long,
    global = true,
    default_value = config::DEFAULT_PATH,
    conflicts_with = "config_stdin"
  )]
  config_file: PathBuf,
  /// Read the settings and secrets from stdin instead, as JSON.
  #[arg(long, global = true)]
  config_stdin: bool,
  /// The channels kick-mod knows.
  #[arg(long, global = true, default_value = channels::DEFAULT_PATH)]
  channels: PathBuf,
  /// The signed-in moderator's token.
  #[arg(long, global = true, default_value = account::DEFAULT_TOKEN_PATH)]
  token: PathBuf,
  /// Every action taken.
  #[arg(long, global = true, default_value = modlog::DEFAULT_PATH)]
  modlog: PathBuf,
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Serve the page. Signing in there redirects back to
  /// http://localhost:<port>/oauth/callback, so the app has to have that
  /// redirect URI, with the port it's served on.
  Serve,
  /// Add channels, by their broadcasters' ids from Kick's official API,
  /// to the channels file. A channel added already is looked up again.
  Add {
    /// Channel slugs, as in their URLs.
    #[arg(required = true)]
    slugs: Vec<String>,
  },
  /// Show the settings, with their secrets left out.
  Config,
}

#[::tokio::main]
async fn main() -> ExitCode {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let args = Args::parse();
  let config = match args.config_stdin {
    true => Source::Stdin,
    false => Source::File(&args.config_file),
  };
  let done = match args.command {
    Command::Serve => {
      let files = Files {
        channels: args.channels,
        token: args.token,
        modlog: args.modlog,
      };
      run(config, files).await
    }
    Command::Add { slugs } => add(config, &args.channels, &slugs).await,
    Command::Config => show(config),
  };
  match done {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("{e}");
      ExitCode::FAILURE
    }
  }
}

/// What `serve` keeps where.
struct Files {
  channels: PathBuf,
  token: PathBuf,
  modlog: PathBuf,
}

async fn run(config: Source<'_>, files: Files) -> Result<(), String> {
  let Files {
    channels,
    token,
    modlog,
  } = files;
  let config = Config::read(config)?;
  let known = channels::load(&channels)?;
  let listen = config.server.listen;
  let redirect = format!("http://localhost:{}/oauth/callback", listen.port());
  let http = ::reqwest::Client::new();
  let account = Account::new(
    http.clone(),
    config.app(),
    api::DEFAULT_BASE,
    redirect,
    token,
  )?;
  let listener = TcpListener::bind(listen)
    .await
    .map_err(|e| format!("listening on {listen}: {e}"))?;
  println!(
    "{} channels; serving at http://localhost:{}",
    known.len(),
    listen.port()
  );
  let logger = logger::Client::new(&config.logger)?;
  println!("chat and history from the logger at {}", config.logger.url);
  let served = Served {
    chat: Chat::start(&known, &logger),
    channels,
    modlog,
    account,
    logger,
  };
  serve::run(listener, served)
    .await
    .map_err(|e| format!("serving: {e}"))
}

async fn add(
  config: Source<'_>,
  path: &Path,
  slugs: &[String],
) -> Result<(), String> {
  let config = Config::read(config)?;
  let api = Api::app(::reqwest::Client::new(), config.app());
  let mut failed = false;
  for slug in slugs.iter().map(|slug| slug.trim()) {
    let broadcaster = match broadcaster(&api, slug).await {
      Ok(id) => id,
      Err(e) => {
        eprintln!("{slug}: {e}");
        failed = true;
        continue;
      }
    };
    channels::update(path, |channels: &mut Channels| {
      channels::record(channels, slug, broadcaster)
    })?;
    println!("{slug}: broadcaster={broadcaster}");
  }
  if failed {
    Err("some channels weren't added".into())
  } else {
    Ok(())
  }
}

/// Prints the settings, as JSON, with `<secret>` for each secret.
fn show(config: Source) -> Result<(), String> {
  let config = Config::read(config)?;
  let shown =
    ::serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
  println!("{shown}");
  Ok(())
}

/// The broadcaster's id for `slug`, from the official API.
async fn broadcaster(api: &Api, slug: &str) -> Result<u64, String> {
  let found = api
    .channels_by_slug(&[slug])
    .await
    .map_err(|e| format!("the official API: {e}"))?;
  match &found[..] {
    [channel] => Ok(channel.broadcaster_user_id),
    _ => Err("the official API has no such channel".into()),
  }
}
