// Probe: stream Kick realtime channels through kick_api::realtime::Client
// and print what arrives, with each publication decoded into its event.
// Usage: cargo run -p kick-api --features client,protocol --example probe --
//          <channel>...
// Channels are Centrifugo's names, such as chatrooms.<chatroom id>.v2.
// An event prints as `ok` or `INCOMPLETE` (in full, with what wasn't
// decoded), or `other` for one kick_api doesn't type. Other replies print
// raw.
use ::kick_api::realtime::{
  Client, Event, Item, Options,
  centrifugo::{Reply, push, reply},
};

#[::tokio::main]
async fn main() {
  let options = Options {
    channels: ::std::env::args().skip(1).collect(),
    ..Options::default()
  };

  let mut client = Client::spawn(options);
  while let Some(item) = client.next().await {
    match item {
      Item::Connected {
        connection,
        endpoint,
      } => println!("connected (#{connection}) to {endpoint}"),
      Item::Frame(frame) => {
        for line in frame.raw().lines().filter(|line| !line.trim().is_empty()) {
          print_reply(line);
        }
      }
      Item::Binary { data, .. } => println!("[binary] {} bytes", data.len()),
      Item::Disconnected {
        connection,
        code,
        reason,
      } => println!("disconnected (#{connection}): {code:?} {reason}"),
      Item::ConnectFailed {
        stage,
        error,
        retry_in,
      } => {
        println!("connect failed at {stage}: {error}; retrying in {retry_in:?}")
      }
      Item::Subscribed { channel, .. } => println!("subscribed to {channel}"),
      Item::Unsubscribed {
        channel,
        code,
        reason,
        ..
      } => println!("unsubscribed from {channel}: {code} {reason}"),
      Item::Stopped { code, reason } => println!("stopped: {code} {reason}"),
    }
  }
}

fn print_reply(line: &str) {
  let reply = match ::serde_json::from_str::<Reply>(line) {
    Ok(reply) => reply,
    Err(error) => return println!("[unreadable reply: {error}] {line}"),
  };
  let Some(reply::Body::Push(push)) = reply.body else {
    return println!("{line}");
  };
  let Some(push::Body::Publication(publication)) = &push.body else {
    return println!("{line}");
  };
  let channel = &push.channel;
  match Event::from_publication(&publication.data) {
    None => println!("{channel}: [not an event] {line}"),
    Some(Event::Other { event, .. }) => println!("{channel}: other {event}"),
    Some(event) if event.complete() => {
      println!("{channel}: ok {}", event.name())
    }
    Some(event) => println!("{channel}: INCOMPLETE {event:#?}"),
  }
}
