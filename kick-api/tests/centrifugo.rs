use {
  ::kick_api::realtime::centrifugo::{
    self, Command, Reply,
    command::{self, ConnectRequest, SubscribeRequest},
    push::{self, Disconnect, Publication, Push},
    reply::{self, ConnectResult, Error},
  },
  ::serde_json::json,
};

/// Frames Kick's Centrifugo sent the probe (2026-11-19), one per line,
/// anonymised (users, channel, ids and uuids): replies, a ping, and one
/// push of each event seen.
const FRAMES: &str = include_str!("fixtures/frames.txt");

fn reply(frame: &str) -> Reply {
  let mut replies = centrifugo::replies(frame);
  let reply = replies.next().unwrap().unwrap();
  assert!(replies.next().is_none());
  reply
}

fn push(frame: &str) -> Push {
  match reply(frame).body {
    Some(reply::Body::Push(push)) => push,
    body => panic!("{body:?}"),
  }
}

#[test]
fn decodes_every_fixture_whole() {
  for frame in FRAMES.lines() {
    let reply = reply(frame);
    assert_eq!(reply.rest, None, "{frame}");
    if let Some(reply::Body::Push(push)) = &reply.body {
      let Some(push::Body::Publication(publication)) = &push.body else {
        panic!("{frame}");
      };
      assert_eq!((&push.rest, &publication.rest), (&None, &None), "{frame}");
    }
  }
}

#[test]
fn connect() {
  let reply = reply(FRAMES.lines().next().unwrap());
  assert_eq!(reply.id, 1);
  assert_eq!(
    reply.body,
    Some(reply::Body::Connect(ConnectResult {
      client: "00000000-0000-4000-8000-000000000001".into(),
      version: "6.9.7 PRO".into(),
      expires: true,
      ttl: 2700,
      ping: 25,
      pong: true,
      ..Default::default()
    }))
  );
}

#[test]
fn subscribe() {
  let reply = reply(r#"{"id":2,"subscribe":{}}"#);
  assert_eq!(reply.id, 2);
  assert_eq!(reply.body, Some(reply::Body::Subscribe(Default::default())));
}

#[test]
fn error() {
  let reply =
    reply(r#"{"id":7,"error":{"code":108,"message":"not available"}}"#);
  assert_eq!(
    (reply.id, reply.error, reply.body),
    (
      7,
      Some(Error {
        code: 108,
        message: "not available".into(),
        ..Default::default()
      }),
      None
    )
  );
}

#[test]
fn ping() {
  assert!(reply("{}").is_ping());
  assert!(!reply(r#"{"id":2,"subscribe":{}}"#).is_ping());
}

#[test]
fn publication() {
  let line = FRAMES
    .lines()
    .find(|line| line.contains("probe 1"))
    .unwrap();
  let push = push(line);
  assert_eq!(push.channel, "chatrooms.1000.v2");
  let Some(push::Body::Publication(Publication { data, tags, .. })) = push.body
  else {
    panic!()
  };
  assert_eq!(data["event"], r"App\Events\ChatMessageEvent");
  assert!(data["data"].as_str().unwrap().contains("probe 1 plain"));
  assert_eq!(tags["event"], r"App\Events\ChatMessageEvent");
}

#[test]
fn batched() {
  let frame = "{}\n{\"id\":2,\"subscribe\":{}}\n";
  let replies = centrifugo::replies(frame)
    .map(Result::unwrap)
    .collect::<Vec<_>>();
  assert_eq!(replies.len(), 2);
  assert!(replies[0].is_ping());
  assert_eq!(replies[1].id, 2);
}

#[test]
fn disconnect() {
  let push =
    push(r#"{"push":{"disconnect":{"code":3501,"reason":"bad request"}}}"#);
  assert_eq!(push.channel, "");
  assert_eq!(
    push.body,
    Some(push::Body::Disconnect(Disconnect {
      code: 3501,
      reason: "bad request".into(),
      rest: None,
    }))
  );
}

#[test]
fn keeps_unknown_fields() {
  let reply = reply(r#"{"id":3,"subscribe":{"new":1},"other":{"x":true}}"#);
  let Some(reply::Body::Subscribe(result)) = &reply.body else {
    panic!()
  };
  assert_eq!(
    result.rest,
    Some(json!({"new": 1}).as_object().unwrap().clone())
  );
  assert_eq!(
    reply.rest,
    Some(json!({"other": {"x": true}}).as_object().unwrap().clone())
  );
}

#[test]
fn keeps_unknown_bodies() {
  // A result this crate doesn't type: the reply decodes, with it in rest.
  let reply = reply(r#"{"id":4,"rpc":{"data":{}}}"#);
  assert_eq!(reply.body, None);
  assert!(reply.rest.unwrap().contains_key("rpc"));
  assert!(!self::reply(r#"{"id":4,"rpc":{}}"#).is_ping());
}

#[test]
fn refuses_two_bodies() {
  let frame = r#"{"id":5,"subscribe":{},"unsubscribe":{}}"#;
  assert!(centrifugo::replies(frame).next().unwrap().is_err());
}

#[test]
fn commands() {
  let connect = Command {
    id: 1,
    body: command::Body::Connect(ConnectRequest {
      token: "token".into(),
      name: "js".into(),
      ..Default::default()
    }),
  };
  assert_eq!(
    connect.encode(),
    r#"{"id":1,"connect":{"token":"token","name":"js"}}"#
  );
  let subscribe = Command {
    id: 2,
    body: command::Body::Subscribe(SubscribeRequest {
      channel: "chatrooms.1000.v2".into(),
      ..Default::default()
    }),
  };
  assert_eq!(
    subscribe.encode(),
    r#"{"id":2,"subscribe":{"channel":"chatrooms.1000.v2"}}"#
  );
}
