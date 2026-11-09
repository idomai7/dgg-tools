use {
  ::dgg_api::chat::{Client, Frame, Item, Options},
  ::futures_util::{SinkExt, StreamExt},
  ::std::time::Duration,
  ::tokio::net::TcpListener,
  ::tokio_tungstenite::tungstenite::Message,
};

#[test]
fn frame_split() {
  let frame = Frame::parse(1, r#"MSG {"nick":"a b"}"#.into());
  assert_eq!(
    (frame.event(), frame.payload()),
    ("MSG", r#"{"nick":"a b"}"#)
  );
  let frame = Frame::parse(1, "PING".into());
  assert_eq!((frame.event(), frame.payload()), ("PING", ""));
  let frame = Frame::parse(1, "X ".into());
  assert_eq!((frame.event(), frame.payload()), ("X", ""));
}

/// Local server: answers PING internally, forwards other frames, and
/// reconnects after the server closes the socket.
#[::tokio::test]
async fn ping_and_reconnect() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let url = format!("ws://{}", listener.local_addr().unwrap());
  let server = ::tokio::spawn(async move {
    for _ in 0..2 {
      let (tcp, _) = listener.accept().await.unwrap();
      let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
      ws.send(Message::text("PING 42")).await.unwrap();
      let Some(Ok(Message::Text(pong))) = ws.next().await else {
        panic!("no pong");
      };
      assert_eq!(pong.as_str(), "PONG 42");
      ws.send(Message::text("MSG {}")).await.unwrap();
      ws.send(Message::binary(vec![1, 2])).await.unwrap();
      ws.close(None).await.unwrap();
    }
  });

  let options = Options {
    url,
    origin: None,
    min_backoff: Duration::from_millis(10),
    ..Options::default()
  };
  let mut client = Client::spawn(options);
  for connection in 1..=2 {
    assert_eq!(client.next().await, Some(Item::Connected { connection }));
    let Some(Item::Frame(frame)) = client.next().await else {
      panic!("expected frame");
    };
    assert_eq!((frame.connection, frame.raw()), (connection, "MSG {}"));
    assert_eq!(
      client.next().await,
      Some(Item::Binary {
        connection,
        data: vec![1, 2]
      })
    );
    let Some(Item::Disconnected {
      connection: closed, ..
    }) = client.next().await
    else {
      panic!("expected disconnect");
    };
    assert_eq!(closed, connection);
  }
  server.await.unwrap();
}
