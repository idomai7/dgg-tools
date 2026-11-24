//! A chat client and moderation tools for Kick channels.

use {
  ::clap::{Parser, Subcommand},
  ::common::config::Source,
  ::kick_api::{
    channel,
    official::{Api, api},
    realtime::Options,
    web::Web,
  },
  ::kick_mod::{
    account::{self, Account},
    channels::{self, Channels},
    chat::{self, Chat, Journal, logger::Feed},
    config::{self, Config},
    modlog,
    serve::{self, Served},
  },
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
  },
  ::tokio::{net::TcpListener, task},
};

/// Names kick-mod, by its codename, to Kick's unofficial API, as it asks.
const USER_AGENT: &str = concat!("gavelgab/", env!("CARGO_PKG_VERSION"));

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
  /// Where chat is journaled, a directory per channel.
  #[arg(long, global = true, default_value = chat::DEFAULT_JOURNAL)]
  journal: PathBuf,
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Serve the page. Signing in there redirects back to
  /// http://localhost:<port>/oauth/callback, so the app has to have that
  /// redirect URI, with the port it's served on.
  Serve,
  /// Look up channels' ids, the broadcaster's and the chatroom's, and
  /// record them in the channels file. Kick's official API has no chatroom
  /// ids, and Cloudflare refuses this program's own HTTP client on the
  /// lookup that has them, so it's asked with curl. If that fails, the
  /// broadcaster's id comes from the official API, and the chatroom id is
  /// left to enter by hand.
  Ids {
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
        journal: args.journal,
      };
      run(config, files).await
    }
    Command::Ids { slugs } => ids(config, &args.channels, &slugs).await,
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
  journal: PathBuf,
}

async fn run(config: Source<'_>, files: Files) -> Result<(), String> {
  let Files {
    channels,
    token,
    modlog,
    journal,
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
  let chat = match &config.logger {
    Some(logger) => {
      println!("live chat from the logger at {}", logger.url);
      Chat::from_logger(&known, &Feed::new(logger)?)
    }
    None => Chat::start(
      &known,
      &Options::default(),
      &Journal::new(&journal, ::common::time::now_ms()),
      Some(Arc::new(Web::new(http.clone(), USER_AGENT))),
    ),
  };
  let served = Served {
    chat,
    channels,
    journal,
    modlog,
    account,
  };
  serve::run(listener, served)
    .await
    .map_err(|e| format!("serving: {e}"))
}

async fn ids(
  config: Source<'_>,
  path: &Path,
  slugs: &[String],
) -> Result<(), String> {
  let mut api = None;
  let mut failed = false;
  for slug in slugs.iter().map(|slug| slug.trim()) {
    let owned = slug.to_owned();
    let looked_up = task::spawn_blocking(move || channel::look_up(&owned))
      .await
      .map_err(|e| e.to_string())?;
    let (broadcaster, ids) = match looked_up {
      Ok(channel) => (channel.user_id, Some((channel.id, channel.chatroom.id))),
      Err(e) => {
        eprintln!("{slug}: the lookup failed: {e}");
        if api.is_none() {
          let config = Config::read(config)?;
          api = Some(Api::app(::reqwest::Client::new(), config.app()));
        }
        match broadcaster(api.as_ref().unwrap(), slug).await {
          Ok(id) => (id, None),
          Err(e) => {
            eprintln!("{slug}: {e}");
            failed = true;
            continue;
          }
        }
      }
    };
    let recorded = channels::update(path, |channels: &mut Channels| {
      let (channel, chatroom) = ids.unzip();
      channels::record(channels, slug, broadcaster, channel, chatroom)
    })?;
    let channel = &recorded[slug];
    let id = |id: Option<u64>| id.map_or("unknown".into(), |id| id.to_string());
    let (channel_id, chatroom) =
      (id(channel.channel_id), id(channel.chatroom_id));
    println!(
      "{slug}: broadcaster={broadcaster} channel={channel_id} \
       chatroom={chatroom}"
    );
    if channel.channel_id.is_none() || channel.chatroom_id.is_none() {
      println!("{slug}: enter the unknown ids in {}", path.display());
    }
  }
  if failed {
    Err("some channels weren't recorded".into())
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
