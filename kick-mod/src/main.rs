//! A chat client and moderation tools for Kick channels.

use {
  ::clap::{Parser, Subcommand},
  ::kick_api::{
    channel,
    official::{Api, api},
    realtime::Options,
  },
  ::kick_mod::{
    account::{self, Account},
    channels::{self, Channels},
    chat::Chat,
    config::{self, Config},
    serve,
  },
  ::std::{
    path::{Path, PathBuf},
    process::ExitCode,
  },
  ::tokio::{net::TcpListener, process::Command as Process},
};

#[derive(Parser)]
#[command(name = "kick-mod")]
struct Args {
  /// The settings.
  #[arg(long, global = true, default_value = config::DEFAULT_PATH)]
  config: PathBuf,
  /// The channels kick-mod knows.
  #[arg(long, global = true, default_value = channels::DEFAULT_PATH)]
  channels: PathBuf,
  /// The signed-in moderator's token.
  #[arg(long, global = true, default_value = account::DEFAULT_TOKEN_PATH)]
  token: PathBuf,
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
}

#[::tokio::main]
async fn main() -> ExitCode {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let args = Args::parse();
  let done = match args.command {
    Command::Serve => run(&args.config, args.channels, args.token).await,
    Command::Ids { slugs } => ids(&args.config, &args.channels, &slugs).await,
  };
  match done {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("{e}");
      ExitCode::FAILURE
    }
  }
}

async fn run(
  config: &Path,
  channels: PathBuf,
  token: PathBuf,
) -> Result<(), String> {
  let config = Config::load(config)?;
  let known = channels::load(&channels)?;
  let listen = config.server.listen;
  let redirect = format!("http://localhost:{}/oauth/callback", listen.port());
  let http = ::reqwest::Client::new();
  let account =
    Account::new(http, config.app(), api::DEFAULT_BASE, redirect, token)?;
  let listener = TcpListener::bind(listen)
    .await
    .map_err(|e| format!("listening on {listen}: {e}"))?;
  println!(
    "{} channels; serving at http://localhost:{}",
    known.len(),
    listen.port()
  );
  let chat = Chat::start(&known, &Options::default());
  serve::run(listener, channels, account, chat)
    .await
    .map_err(|e| format!("serving: {e}"))
}

async fn ids(
  config: &Path,
  path: &Path,
  slugs: &[String],
) -> Result<(), String> {
  let mut api = None;
  let mut failed = false;
  for slug in slugs.iter().map(|slug| slug.trim()) {
    let (broadcaster, chatroom) = match look_up(slug).await {
      Ok(channel) => (channel.user_id, Some(channel.chatroom.id)),
      Err(e) => {
        eprintln!("{slug}: the lookup failed: {e}");
        if api.is_none() {
          let config = Config::load(config)?;
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
      channels::record(channels, slug, broadcaster, chatroom)
    })?;
    let channel = &recorded[slug];
    match channel.chatroom_id {
      Some(chatroom) => {
        println!("{slug}: broadcaster={broadcaster} chatroom={chatroom}")
      }
      None => println!(
        "{slug}: broadcaster={broadcaster} chatroom unknown: enter it in {}",
        path.display()
      ),
    }
  }
  if failed {
    Err("some channels weren't recorded".into())
  } else {
    Ok(())
  }
}

/// Asks the unofficial lookup for `slug` with curl.
async fn look_up(slug: &str) -> Result<channel::Channel, String> {
  let output = Process::new("curl")
    .args(["-sS", "-m", "20", "-H", "Accept: application/json"])
    .arg(channel::url(slug))
    .output()
    .await
    .map_err(|e| format!("running curl: {e}"))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(format!("curl: {}", stderr.trim()));
  }
  let body = String::from_utf8_lossy(&output.stdout);
  channel::parse(&body).map_err(|e| format!("{e}: {}", body.trim()))
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
