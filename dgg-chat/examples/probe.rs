// Probe: connect to dgg chat and print MSG events until the socket closes.
// Usage: cargo run -p dgg-chat --example probe -- [--no-origin]
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{
  Message, client::IntoClientRequest, http::HeaderValue,
};

#[tokio::main]
async fn main() {
  let _ = rustls::crypto::ring::default_provider().install_default();
  let mut req = "wss://chat.destiny.gg/ws".into_client_request().unwrap();
  if !std::env::args().any(|a| a == "--no-origin") {
    req
      .headers_mut()
      .insert("Origin", HeaderValue::from_static("https://www.destiny.gg"));
  }

  let (mut ws, resp) = match tokio_tungstenite::connect_async(req).await {
    Ok(v) => v,
    Err(e) => return eprintln!("connect failed: {e}"),
  };
  println!("connected: {}", resp.status());

  while let Some(frame) = ws.next().await {
    let Ok(Message::Text(text)) = frame else {
      continue;
    };
    let (event, payload) = text.split_once(' ').unwrap_or((&text, ""));
    match event {
      "PING" => ws
        .send(Message::text(format!("PONG {payload}")))
        .await
        .unwrap(),
      "MSG" => println!("{payload}"),
      _ => println!("[{event}] {} bytes", payload.len()),
    }
  }
  println!("disconnected");
}
