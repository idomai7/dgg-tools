// Sign-in: sign in to Kick's official API for real, and check it against
// realtime chat.
// Usage: cargo run -p kick-api --features official,client,protocol
//          --example signin -- <token file> [<chatroom id>]
// The app's credentials come from KICK_CLIENT_ID and KICK_CLIENT_SECRET,
// and its redirect URI must be http://localhost:8787/oauth/callback.
//
// Without a token in <token file>, it prints a sign-in URL, waits for the
// redirect, and saves the token there (keep it out of git, as in data/).
// Then it names the signed-in user and their channel. With the chatroom id
// of that channel, it sends a message there, waits for it in realtime chat,
// deletes it, and waits for the deletion: printing whether the ids match.
mod common;

use {
  ::kick_api::{
    official::{
      Api,
      api::{Chat, Sender},
      oauth::{App, Token},
    },
    realtime::{Client, Event, Item, Options},
  },
  ::std::{
    env, fs,
    time::{SystemTime, UNIX_EPOCH},
  },
  ::tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
  },
  common::{event, save, wait},
};

const REDIRECT: &str = "http://localhost:8787/oauth/callback";
const SCOPES: &[&str] = &[
  "user:read",
  "channel:read",
  "chat:write",
  "moderation:ban",
  "moderation:chat_message:manage",
];

#[::tokio::main]
async fn main() {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let mut args = env::args().skip(1);
  let path = args.next().expect("usage: <token file> [<chatroom id>]");
  let chatroom: Option<u64> =
    args.next().map(|id| id.parse().expect("a chatroom id"));
  let var = |name| env::var(name).unwrap_or_else(|_| panic!("set {name}"));
  let app = App::new(var("KICK_CLIENT_ID"), var("KICK_CLIENT_SECRET"));
  let http = ::reqwest::Client::new();

  let token = match fs::read_to_string(&path) {
    Ok(saved) => ::serde_json::from_str(&saved).expect("a saved token"),
    Err(_) => sign_in(&app, &http).await,
  };
  save(&path, &token);
  println!("token: {:?}", token.scopes().collect::<Vec<_>>());
  let kept = path.clone();
  let api = Api::user(http, app, token, move |token| {
    println!("refreshed the token");
    save(&kept, token);
  });

  let [me] = &api.users(&[]).await.expect("the user")[..] else {
    panic!("not one user");
  };
  println!("user: {} {:?}", me.user_id, me.name);
  let [channel] = &api.channels_by_id(&[]).await.expect("the channel")[..]
  else {
    panic!("not one channel");
  };
  println!(
    "channel: {} {:?}",
    channel.broadcaster_user_id, channel.slug
  );
  if let Some(chatroom) = chatroom {
    check(&api, me.user_id, channel.broadcaster_user_id, chatroom).await;
  }
}

/// Signs in through the browser: the token for the user who did.
async fn sign_in(app: &App, http: &::reqwest::Client) -> Token {
  let sign_in = app.sign_in(REDIRECT, SCOPES);
  let listener = TcpListener::bind("127.0.0.1:8787").await.unwrap();
  println!("sign in at:\n{}", sign_in.url);
  let code = loop {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut read = vec![0; 8192];
    let n = socket.read(&mut read).await.unwrap();
    let request = String::from_utf8_lossy(&read[..n]);
    let target = request.split(' ').nth(1).unwrap_or_default();
    let Some(query) = target.strip_prefix("/oauth/callback?") else {
      let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
      continue;
    };
    let code = sign_in.code(query);
    let page = match &code {
      Ok(_) => "Signed in: back to the terminal.".to_owned(),
      Err(refused) => format!("Not signed in: {refused:?}"),
    };
    let answer = format!(
      "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\
       content-length: {}\r\nconnection: close\r\n\r\n{page}",
      page.len()
    );
    let _ = socket.write_all(answer.as_bytes()).await;
    break code.expect("a code");
  };
  app
    .exchange(http, REDIRECT, &sign_in.verifier, &code)
    .await
    .expect("a token")
}

/// Sends a message to the chatroom, and deletes it, checking both arrive
/// in realtime chat with the official API's ids.
async fn check(api: &Api, user_id: u64, broadcaster_user_id: u64, room: u64) {
  let mut client = Client::spawn(Options {
    channels: vec![format!("chatrooms.{room}.v2")],
    ..Options::default()
  });
  let subscribed = wait(&mut client, |item| match item {
    Item::Subscribed { channel, .. } => Some(channel),
    _ => None,
  })
  .await
  .expect("subscribed");
  println!("subscribed: {subscribed}");

  let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
  let chat = Chat {
    content: format!("kick-api check {}", now.as_millis()),
    r#type: Sender::User,
    broadcaster_user_id: Some(broadcaster_user_id),
    reply_to_message_id: None,
  };
  let sent = api.send_chat(&chat).await.expect("sent");
  println!("sent: {} {:?}", sent.is_sent, sent.message_id);
  let message = wait(&mut client, |item| match event(item)? {
    Event::ChatMessageEvent(message) if message.content == chat.content => {
      Some(message)
    }
    _ => None,
  })
  .await
  .expect("the message");
  println!(
    "message id matches: {}",
    sent.message_id.as_ref() == Some(&message.id)
  );
  println!("sender id matches: {}", message.sender.id == user_id);

  api.delete_message(&message.id).await.expect("deleted");
  let deleted = wait(&mut client, |item| match event(item)? {
    Event::MessageDeletedEvent(deleted) => Some(deleted),
    _ => None,
  })
  .await
  .expect("the deletion");
  println!("deleted id matches: {}", deleted.message.id == message.id);
}
