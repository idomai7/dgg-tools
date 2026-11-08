// Probe: stream dgg chat through dgg_chat::Client and print what arrives.
// Usage: cargo run -p dgg-chat --example probe -- [--no-origin]
use ::dgg_chat::{Client, Item, Options};

#[::tokio::main]
async fn main() {
  let mut opts = Options::default();
  if ::std::env::args().any(|a| a == "--no-origin") {
    opts.origin = None;
  }

  let mut client = Client::spawn(opts);
  while let Some(item) = client.next().await {
    match item {
      Item::Connected { connection } => println!("connected (#{connection})"),
      Item::Frame(f) if f.event() == "MSG" => println!("{}", f.payload()),
      Item::Frame(f) => println!("[{}] {} bytes", f.event(), f.payload().len()),
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
