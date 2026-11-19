//! A chat client and moderation tools for Kick channels.

use {
  ::clap::{Parser, Subcommand},
  ::kick_api::channel::{self, Channel},
  ::std::process::{Command as Process, ExitCode},
};

#[derive(Parser)]
#[command(name = "kick-mod")]
struct Args {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Look up channels' ids: the channel, its chatroom and its broadcaster.
  /// Kick's official API has no chatroom ids, and Cloudflare refuses this
  /// program's own HTTP client on the lookup that has them, so it's asked
  /// with curl.
  Ids {
    /// Channel slugs, as in their URLs.
    #[arg(
      env = "KICK_CHANNELS",
      value_delimiter = ',',
      required = true,
      hide_env_values = true
    )]
    slugs: Vec<String>,
  },
}

fn main() -> ExitCode {
  let _ = ::dotenvy::dotenv();
  match Args::parse().command {
    Command::Ids { slugs } => ids(&slugs),
  }
}

fn ids(slugs: &[String]) -> ExitCode {
  let mut code = ExitCode::SUCCESS;
  for slug in slugs.iter().map(|slug| slug.trim()) {
    match look_up(slug) {
      Ok(channel) => println!(
        "{} channel={} chatroom={} user={}",
        channel.slug, channel.id, channel.chatroom.id, channel.user_id
      ),
      Err(e) => {
        eprintln!("{slug}: {e}");
        code = ExitCode::FAILURE;
      }
    }
  }
  code
}

/// Asks the lookup for `slug` with curl.
fn look_up(slug: &str) -> Result<Channel, String> {
  let output = Process::new("curl")
    .args(["-sS", "-m", "20", "-H", "Accept: application/json"])
    .arg(channel::url(slug))
    .output()
    .map_err(|e| format!("running curl: {e}"))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(format!("curl: {}", stderr.trim()));
  }
  let body = String::from_utf8_lossy(&output.stdout);
  channel::parse(&body).map_err(|e| format!("{e}: {}", body.trim()))
}
