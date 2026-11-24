//! Look up Kick channels' ids into the channel list.

use {
  super::block_on,
  crate::{USER_AGENT, kick::channels},
  ::clap::Args,
  ::kick_api::channel,
  ::std::{path::Path, process::ExitCode},
};

/// Look up Kick channels' ids, the channel's and the chatroom's, with
/// Kick's unofficial lookup (anonymously), and record them in
/// the install's `kick-channels`, the Kick channels to capture. A channel
/// the lookup fails for can be added by hand: `<slug> <channel id>
/// <chatroom id>`.
#[derive(Args)]
pub struct KickIds {
  /// The channels, as in their URLs.
  #[arg(required = true)]
  slugs: Vec<String>,
}

impl KickIds {
  /// Records them in `list`.
  pub fn run(self, list: &Path) -> ExitCode {
    block_on(self.look_up(list))
  }

  async fn look_up(self, list: &Path) -> ExitCode {
    let _ = ::rustls::crypto::ring::default_provider().install_default();
    let http = ::reqwest::Client::new();
    let mut failed = false;
    for slug in &self.slugs {
      let ask =
        async |slug: &str| channel::look_up(&http, USER_AGENT, slug).await;
      let recorded = channels::look_up(slug, ask)
        .await
        .and_then(|found| channels::update(list, &found).map(|()| found));
      match recorded {
        Ok(found) => println!(
          "{}: channel={} chatroom={}",
          found.slug, found.channel_id, found.chatroom_id
        ),
        Err(e) => {
          eprintln!("{slug}: {e}");
          failed = true;
        }
      }
    }
    if failed {
      eprintln!(
        "add what's missing to {} by hand: `<slug> <channel id> <chatroom \
         id>`",
        list.display()
      );
      ExitCode::FAILURE
    } else {
      println!("recorded in {}", list.display());
      ExitCode::SUCCESS
    }
  }
}
