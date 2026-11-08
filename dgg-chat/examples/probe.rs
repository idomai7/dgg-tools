// Probe: stream dgg chat through dgg_chat::Client and print what arrives.
// Usage: cargo run -p dgg-chat --example probe -- [--no-origin]
use ::dgg_chat::{Client, Item, Options};

#[::tokio::main]
async fn main() {
  let mut options = Options::default();
  if ::std::env::args().any(|arg| arg == "--no-origin") {
    options.origin = None;
  }

  let mut client = Client::spawn(options);
  while let Some(item) = client.next().await {
    match item {
      Item::Connected { connection } => println!("connected (#{connection})"),
      Item::Frame(frame) if frame.event() == "MSG" => {
        println!("{}", frame.payload())
      }
      Item::Frame(frame) => {
        println!("[{}] {} bytes", frame.event(), frame.payload().len())
      }
      Item::Binary { data, .. } => println!("[binary] {} bytes", data.len()),
      Item::Disconnected { connection, reason } => {
        println!("disconnected (#{connection}): {reason}")
      }
      Item::ConnectFailed { error, retry_in } => {
        println!("connect failed: {error}; retrying in {retry_in:?}")
      }
    }
  }
}
