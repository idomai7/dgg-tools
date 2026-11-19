//! The realtime client against a local server, and Kick's setup answers.
//! Ids and tokens are made up.

use {
  ::futures_util::{SinkExt, StreamExt},
  ::kick_api::realtime::{Client, Item, Options, Setup, Stage, connection},
  ::std::time::Duration,
  ::tokio::net::{TcpListener, TcpStream},
  ::tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
      Message,
      protocol::{CloseFrame, frame::coding::CloseCode},
    },
  },
};

const CHANNEL: &str = "chatrooms.1000.v2";

#[test]
fn decodes_connection() {
  let answer = r#"{"data":{"connections":[{"provider":"centrifugo",
    "credentials":{"url":"wss://realtime.example/connection/websocket"}}],
    "mode":"websocket"},"message":"success"}"#;
  assert_eq!(
    connection::decode_connection(answer).unwrap(),
    "wss://realtime.example/connection/websocket"
  );
  let none = r#"{"data":{"connections":[{"provider":"pusher"}]}}"#;
  assert!(connection::decode_connection(none).is_err());
}

#[test]
fn decodes_token() {
  let answer = r#"{"data":{"token":"t0k3n"},"message":"success"}"#;
  assert_eq!(connection::decode_token(answer).unwrap(), "t0k3n");
  assert!(connection::decode_token("{}").is_err());
}

#[test]
fn client_ids_are_uuids() {
  let id = connection::client_id();
  let groups: Vec<_> = id.split('-').map(str::len).collect();
  assert_eq!(groups, [8, 4, 4, 4, 12]);
  assert_eq!(&id[14..15], "4");
  assert!("89ab".contains(&id[19..20]));
  assert_ne!(id, connection::client_id());
}

type Ws = WebSocketStream<TcpStream>;

async fn listen() -> (TcpListener, String) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let url = format!("ws://{}", listener.local_addr().unwrap());
  (listener, url)
}

fn options(endpoint: &str, channels: &[&str]) -> Options {
  Options {
    setup: Setup::Fixed {
      endpoint: endpoint.into(),
      token: "t0k3n".into(),
    },
    channels: channels.iter().map(|&c| c.into()).collect(),
    min_backoff: Duration::from_millis(10),
    ..Options::default()
  }
}

async fn receive(ws: &mut Ws) -> String {
  match ws.next().await {
    Some(Ok(Message::Text(text))) => text.as_str().to_owned(),
    other => panic!("expected text, got {other:?}"),
  }
}

/// Accepts a connection and answers its connect command.
async fn accept(listener: &TcpListener) -> Ws {
  let (tcp, _) = listener.accept().await.unwrap();
  let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
  assert_eq!(
    receive(&mut ws).await,
    r#"{"id":1,"connect":{"token":"t0k3n","name":"js"}}"#
  );
  ws.send(Message::text(CONNECTED)).await.unwrap();
  ws
}

const CONNECTED: &str =
  r#"{"id":1,"connect":{"client":"c1","version":"6","ping":25,"pong":true}}"#;

async fn send(ws: &mut Ws, text: &str) {
  ws.send(Message::text(text.to_owned())).await.unwrap();
}

async fn close(ws: &mut Ws, code: u16, reason: &str) {
  let frame = CloseFrame {
    code: CloseCode::from(code),
    reason: reason.to_owned().into(),
  };
  ws.close(Some(frame)).await.unwrap();
}

async fn frame(client: &mut Client, connection: u64, raw: &str) {
  match client.next().await {
    Some(Item::Frame(frame)) => {
      assert_eq!((frame.connection, frame.raw()), (connection, raw))
    }
    other => panic!("expected frame {raw}, got {other:?}"),
  }
}

/// Connects, subscribes, answers pings without yielding them, resubscribes
/// a channel unsubscribed with 2500+, reconnects after a close, and stops
/// on a disconnect of 3500+.
#[::tokio::test]
async fn session() {
  let (listener, url) = listen().await;
  let server = ::tokio::spawn(async move {
    let mut ws = accept(&listener).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":2,"subscribe":{"channel":"chatrooms.1000.v2"}}"#
    );
    send(&mut ws, r#"{"id":2,"subscribe":{}}"#).await;
    send(&mut ws, "{}").await;
    assert_eq!(receive(&mut ws).await, "{}");
    send(&mut ws, PUBLICATION).await;
    ws.send(Message::binary(vec![1, 2])).await.unwrap();
    send(&mut ws, UNSUBSCRIBE).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":3,"subscribe":{"channel":"chatrooms.1000.v2"}}"#
    );
    send(&mut ws, r#"{"id":3,"subscribe":{}}"#).await;
    close(&mut ws, 3001, "shutdown").await;

    let mut ws = accept(&listener).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":2,"subscribe":{"channel":"chatrooms.1000.v2"}}"#
    );
    send(&mut ws, DISCONNECT).await;
  });

  let mut client = Client::spawn(options(&url, &[CHANNEL]));
  let subscribed = |connection| Item::Subscribed {
    connection,
    channel: CHANNEL.into(),
  };
  assert_eq!(
    client.next().await,
    Some(Item::Connected {
      connection: 1,
      endpoint: url.clone()
    })
  );
  frame(&mut client, 1, CONNECTED).await;
  frame(&mut client, 1, r#"{"id":2,"subscribe":{}}"#).await;
  assert_eq!(client.next().await, Some(subscribed(1)));
  frame(&mut client, 1, PUBLICATION).await;
  assert_eq!(
    client.next().await,
    Some(Item::Binary {
      connection: 1,
      data: vec![1, 2]
    })
  );
  frame(&mut client, 1, UNSUBSCRIBE).await;
  assert_eq!(
    client.next().await,
    Some(Item::Unsubscribed {
      connection: 1,
      channel: CHANNEL.into(),
      code: 2500,
      reason: "insufficient state".into()
    })
  );
  frame(&mut client, 1, r#"{"id":3,"subscribe":{}}"#).await;
  assert_eq!(client.next().await, Some(subscribed(1)));
  assert_eq!(
    client.next().await,
    Some(Item::Disconnected {
      connection: 1,
      code: Some(3001),
      reason: "shutdown".into()
    })
  );

  assert_eq!(
    client.next().await,
    Some(Item::Connected {
      connection: 2,
      endpoint: url
    })
  );
  frame(&mut client, 2, CONNECTED).await;
  frame(&mut client, 2, DISCONNECT).await;
  assert_eq!(
    client.next().await,
    Some(Item::Stopped {
      code: 3501,
      reason: "bad request".into()
    })
  );
  assert_eq!(client.next().await, None);
  server.await.unwrap();
}

const PUBLICATION: &str = r#"{"push":{"channel":"chatrooms.1000.v2","pub":{"data":{"event":"App\\Events\\ChatroomClearEvent","data":"{\"id\":\"x\"}"}}}}"#;
const UNSUBSCRIBE: &str = r#"{"push":{"channel":"chatrooms.1000.v2","unsubscribe":{"code":2500,"reason":"insufficient state"}}}"#;
const DISCONNECT: &str =
  r#"{"push":{"disconnect":{"code":3501,"reason":"bad request"}}}"#;

/// A refused subscribe isn't asked again on the next connection.
#[::tokio::test]
async fn refused_subscribe_stays_a_gap() {
  let (listener, url) = listen().await;
  let server = ::tokio::spawn(async move {
    let mut ws = accept(&listener).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":2,"subscribe":{"channel":"a"}}"#
    );
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":3,"subscribe":{"channel":"b"}}"#
    );
    send(&mut ws, r#"{"id":2,"subscribe":{}}"#).await;
    send(&mut ws, REFUSED).await;
    close(&mut ws, 3000, "").await;

    let mut ws = accept(&listener).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":2,"subscribe":{"channel":"a"}}"#
    );
    close(&mut ws, 3500, "invalid token").await;
  });

  let mut client = Client::spawn(options(&url, &["b", "a"]));
  let mut items = Vec::new();
  while let Some(item) = client.next().await {
    if !matches!(item, Item::Frame(_)) {
      items.push(item);
    }
  }
  assert_eq!(
    items,
    [
      Item::Connected {
        connection: 1,
        endpoint: url.clone()
      },
      Item::Subscribed {
        connection: 1,
        channel: "a".into()
      },
      Item::Unsubscribed {
        connection: 1,
        channel: "b".into(),
        code: 103,
        reason: "permission denied".into()
      },
      Item::Disconnected {
        connection: 1,
        code: Some(3000),
        reason: "".into()
      },
      Item::Connected {
        connection: 2,
        endpoint: url
      },
      Item::Stopped {
        code: 3500,
        reason: "invalid token".into()
      },
    ]
  );
  server.await.unwrap();
}

const REFUSED: &str =
  r#"{"id":3,"error":{"code":103,"message":"permission denied"}}"#;

/// A connect command answered with an error is a failed connect, retried.
#[::tokio::test]
async fn refused_connect() {
  let (listener, url) = listen().await;
  let server = ::tokio::spawn(async move {
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = ::tokio_tungstenite::accept_async(tcp).await.unwrap();
    receive(&mut ws).await;
    send(
      &mut ws,
      r#"{"id":1,"error":{"code":109,"message":"token expired"}}"#,
    )
    .await;
    accept(&listener).await
  });

  let mut client = Client::spawn(options(&url, &[]));
  let Some(Item::ConnectFailed { stage, error, .. }) = client.next().await
  else {
    panic!("expected a failed connect");
  };
  assert_eq!(
    (stage, error.as_str()),
    (Stage::ConnectReply, "109 token expired")
  );
  assert!(matches!(
    client.next().await,
    Some(Item::Connected { connection: 1, .. })
  ));
  server.await.unwrap();
}

/// Channels added and dropped while connected are subscribed and
/// unsubscribed then.
#[::tokio::test]
async fn subscribes_on_request() {
  let (listener, url) = listen().await;
  let server = ::tokio::spawn(async move {
    let mut ws = accept(&listener).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":2,"subscribe":{"channel":"a"}}"#
    );
    send(&mut ws, r#"{"id":2,"subscribe":{}}"#).await;
    assert_eq!(
      receive(&mut ws).await,
      r#"{"id":3,"unsubscribe":{"channel":"a"}}"#
    );
    send(&mut ws, r#"{"id":3,"unsubscribe":{}}"#).await;
    ws
  });

  let mut client = Client::spawn(options(&url, &[]));
  assert!(matches!(client.next().await, Some(Item::Connected { .. })));
  frame(&mut client, 1, CONNECTED).await;
  client.subscribe("a");
  frame(&mut client, 1, r#"{"id":2,"subscribe":{}}"#).await;
  assert_eq!(
    client.next().await,
    Some(Item::Subscribed {
      connection: 1,
      channel: "a".into()
    })
  );
  client.unsubscribe("a");
  frame(&mut client, 1, r#"{"id":3,"unsubscribe":{}}"#).await;
  assert!(matches!(
    client.next().await,
    Some(Item::Unsubscribed { code: 0, .. })
  ));
  server.await.unwrap();
}
