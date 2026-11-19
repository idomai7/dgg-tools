// Probe: stream Kick realtime channels through kick_api::realtime::Client
// and print what arrives.
// Usage: cargo run -p kick-api --features client --example probe --
//          <channel>...
// Channels are Centrifugo's names, such as chatrooms.<chatroom id>.v2.
use ::kick_api::realtime::{Client, Item, Options};

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
      Item::Frame(frame) => println!("{}", frame.raw()),
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
