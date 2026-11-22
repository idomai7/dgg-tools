// Bot: send chat as a bot rather than as the user, and see how it arrives.
// Usage: cargo run -p kick-api --features official,client,protocol
//          --example bot -- <token file> <chatroom id>
// The app's credentials come from KICK_CLIENT_ID and KICK_CLIENT_SECRET,
// and <token file> is one the signin example saved. <chatroom id> is the
// signed-in user's channel's.
//
// It names the app first, as Kick sees it. Then it sends `type: bot`
// twice: with the user's token, then with the app's own. For each it
// prints Kick's answer and the sender realtime chat shows, then deletes
// the message with the user's token.
mod common;

use {
  ::kick_api::{
    official::{
      Api,
      api::{Chat, Sender},
      oauth::App,
    },
    realtime::{Client, Event, Item, Options},
  },
  ::std::{
    env, fs,
    time::{SystemTime, UNIX_EPOCH},
  },
  common::{event, save, show_app, wait},
};

#[::tokio::main]
async fn main() {
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let mut args = env::args().skip(1);
  let usage = "usage: <token file> <chatroom id>";
  let path = args.next().expect(usage);
  let room: u64 = args.next().expect(usage).parse().expect("a chatroom id");
  let var = |name| env::var(name).unwrap_or_else(|_| panic!("set {name}"));
  let app = App::new(var("KICK_CLIENT_ID"), var("KICK_CLIENT_SECRET"));
  let http = ::reqwest::Client::new();
  show_app(&app, &http).await;

  let saved = fs::read_to_string(&path).expect("a token: run signin first");
  let token = ::serde_json::from_str(&saved).expect("a saved token");
  let kept = path.clone();
  let user = Api::user(http.clone(), app.clone(), token, move |token| {
    println!("refreshed the token");
    save(&kept, token);
  });
  let as_app = Api::app(http, app);
  let [channel] = &user.channels_by_id(&[]).await.expect("the channel")[..]
  else {
    panic!("not one channel");
  };
  println!(
    "channel: {} {:?}",
    channel.broadcaster_user_id, channel.slug
  );

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

  for (whose, api) in [("user's", &user), ("app's", &as_app)] {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let chat = Chat {
      content: format!("kick-api bot check {}", now.as_millis()),
      r#type: Sender::Bot,
      broadcaster_user_id: Some(channel.broadcaster_user_id),
      reply_to_message_id: None,
    };
    println!("\nas a bot, with the {whose} token:");
    let sent = match api.send_chat(&chat).await {
      Ok(sent) => sent,
      Err(error) => {
        println!("  refused: {error}");
        continue;
      }
    };
    println!("  sent: {} {:?}", sent.is_sent, sent.message_id);
    let message = wait(&mut client, |item| match event(item)? {
      Event::ChatMessageEvent(message) if message.content == chat.content => {
        Some(message)
      }
      _ => None,
    })
    .await;
    let Some(message) = message else {
      println!("  didn't arrive in this chatroom");
      continue;
    };
    let sender = &message.sender;
    println!(
      "  arrived from: {} {} {:?}",
      sender.id, sender.username, sender.identity
    );
    match user.delete_message(&message.id).await {
      Ok(()) => println!("  deleted"),
      Err(error) => println!("  not deleted: {error}"),
    }
  }
}
