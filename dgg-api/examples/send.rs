// Send: log in to dgg chat with a login key, then send one command and show
// chat's answer. With no command, only checks the key logs in.
//
// The key: on destiny.gg, Profile > Developer > Connections > "Add login
// key". Chat logs in whoever connects with the cookie `authtoken=<key>`.
// Put it in DGG_LOGIN_KEY, in the environment or in .env (git-ignored).
// Anyone with the key can chat as you: keep it secret, and remove it there
// when done.
//
// Usage: cargo run -p dgg-api --features client,protocol --example send --
//          [msg <text> | whisper <nick> <text>]
// This posts to the real chat, as you.

use {
  ::dgg_api::chat::{
    Client, Item, Options,
    protocol::{
      Command, Event,
      event::{Error, PrivMsg},
    },
  },
  ::std::{env, process::ExitCode, time::Duration},
  ::tokio::time::{Instant, timeout_at},
};

/// How long to wait for chat to log in, and then to answer.
const PATIENCE: Duration = Duration::from_secs(15);

/// The command in `args`, if any; `Err` with the usage if they're wrong.
fn command(args: &[String]) -> Result<Option<Command>, String> {
  let usage = "usage: send [msg <text> | whisper <nick> <text>]";
  match args {
    [] => Ok(None),
    [kind, text @ ..] if kind == "msg" && !text.is_empty() => {
      Ok(Some(Command::Msg(text.join(" "))))
    }
    [kind, nick, text @ ..] if kind == "whisper" && !text.is_empty() => {
      Ok(Some(Command::PrivMsg {
        nick: nick.clone(),
        data: text.join(" "),
      }))
    }
    _ => Err(usage.into()),
  }
}

/// Whether `key` looks like a login key: what chat accepts as one.
fn valid_key(key: &str) -> bool {
  (10..=64).contains(&key.len())
    && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[::tokio::main]
async fn main() -> ExitCode {
  let args: Vec<String> = env::args().skip(1).collect();
  let command = match command(&args) {
    Ok(command) => command,
    Err(usage) => {
      eprintln!("{usage}");
      return ExitCode::FAILURE;
    }
  };
  let _ = ::dotenvy::dotenv();
  let Ok(key) = env::var("DGG_LOGIN_KEY") else {
    eprintln!(
      "DGG_LOGIN_KEY isn't set: make a login key on destiny.gg (Profile > \
       Developer > Connections) and put it in the environment or .env"
    );
    return ExitCode::FAILURE;
  };
  if !valid_key(&key) {
    eprintln!("DGG_LOGIN_KEY isn't a login key (10 to 64 letters and digits)");
    return ExitCode::FAILURE;
  }

  let mut client = Client::spawn(Options {
    cookie: Some(format!("authtoken={key}")),
    ..Options::default()
  });
  let mut deadline = Instant::now() + PATIENCE;
  let mut me = None;
  loop {
    let item = match timeout_at(deadline, client.next()).await {
      Ok(Some(item)) => item,
      Ok(None) => return ExitCode::FAILURE,
      Err(_) if me.is_none() => {
        eprintln!("chat didn't say who's logged in");
        return ExitCode::FAILURE;
      }
      Err(_) => {
        eprintln!("no answer from chat (it confirms only whispers)");
        return ExitCode::FAILURE;
      }
    };
    let frame = match item {
      Item::Frame(frame) => frame,
      Item::ConnectFailed { error, .. } => {
        eprintln!("connect failed: {error}");
        return ExitCode::FAILURE;
      }
      Item::Disconnected { reason, .. } => {
        eprintln!("disconnected: {reason}");
        return ExitCode::FAILURE;
      }
      _ => continue,
    };
    match (Event::parse(frame.raw()), &me, &command) {
      (Event::Me(None), _, _) => {
        eprintln!("not logged in: chat didn't accept the key");
        return ExitCode::FAILURE;
      }
      (Event::Me(Some(user)), _, _) => {
        println!("logged in as {}", user.nick);
        let Some(command) = &command else {
          return ExitCode::SUCCESS;
        };
        let frame = command.encode();
        println!("> {frame}");
        if let Err(e) = client.send(frame) {
          eprintln!("{e}");
          return ExitCode::FAILURE;
        }
        me = Some(user.nick);
        deadline = Instant::now() + PATIENCE;
      }
      (Event::Err(Error { description, .. }), Some(_), _) => {
        eprintln!("refused: {description:?}");
        return ExitCode::FAILURE;
      }
      (Event::PrivMsgSent, Some(_), Some(Command::PrivMsg { .. })) => {
        println!("whisper delivered");
        return ExitCode::SUCCESS;
      }
      (Event::Msg(message), Some(nick), Some(Command::Msg(text)))
        if message.user.nick == *nick && message.data == *text =>
      {
        println!("chat echoed it at {}", message.timestamp);
        return ExitCode::SUCCESS;
      }
      // A whisper to yourself comes back too.
      (Event::PrivMsg(PrivMsg { nick, data, .. }), Some(_), _) => {
        println!("whisper from {nick}: {data}");
      }
      _ => {}
    }
  }
}
