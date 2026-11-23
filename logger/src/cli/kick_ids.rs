//! Look up Kick channels' ids into the channel list.

use {
  crate::kick::channels,
  ::clap::Args,
  ::kick_api::channel,
  ::std::{path::Path, process::ExitCode},
};

/// Look up Kick channels' ids, the channel's and the chatroom's, with
/// Kick's unofficial lookup (anonymously, with curl), and record them in
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
    let mut failed = false;
    for slug in &self.slugs {
      let recorded = channels::look_up(slug, channel::look_up)
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
