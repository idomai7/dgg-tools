//! Capture: dgg chat into the journal, and nothing else, so it stays small
//! and rarely needs upgrading.

use {
  ::dgg_chat::{Client, Item, Options},
  ::journal::{Writer, record::v1::Body, writer::SYNC_INTERVAL},
  ::std::{
    io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
  },
};

/// The [`Body::Close`] reason written when capture shuts down cleanly.
pub const SHUTDOWN: &str = "shutdown";

/// An instance id unique to this run: `<host>-<start unix ms>`, with any
/// character a journal instance id can't hold replaced by `-`.
pub fn instance_id(host: &str, start_ms: i64) -> String {
  let host: String = host
    .chars()
    .map(|c| match c {
      'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => c,
      _ => '-',
    })
    .collect();
  let host = if host.is_empty() { "host" } else { &host };
  format!("{host}-{start_ms}")
}

/// Unix ms now.
pub fn now_ms() -> i64 {
  let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
  since.as_millis() as i64
}

/// Streams chat into `journal_root` as `instance` until `shutdown`
/// completes, then closes the open connection's record and syncs.
///
/// Each item is stamped with its receive time as it's taken off the client's
/// queue. A journal write error ends the run, since carrying on would
/// silently drop messages.
pub async fn run(
  chat: Options,
  journal_root: PathBuf,
  instance: String,
  shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
  let url = chat.url.clone();
  let mut writer = Writer::new(journal_root, instance);
  let mut client = Client::spawn(chat);
  let mut sync = ::tokio::time::interval(SYNC_INTERVAL);
  let mut connected = None;
  ::tokio::pin!(shutdown);
  loop {
    ::tokio::select! {
      item = client.next() => {
        let Some(item) = item else { break };
        let now = now_ms();
        match item {
          Item::Connected { connection } => {
            connected = Some(connection);
            writer.write(now, connection, Body::Open(url.clone()))?;
          }
          Item::Frame(frame) => {
            let connection = frame.connection;
            writer.write(now, connection, Body::Frame(frame.into_raw()))?;
          }
          Item::Binary { connection, data } => {
            writer.write(now, connection, Body::Binary(data))?;
          }
          Item::Disconnected { connection, reason } => {
            connected = None;
            eprintln!("disconnected (#{connection}): {reason}");
            writer.write(now, connection, Body::Close(reason))?;
          }
          Item::ConnectFailed { error, retry_in } => {
            eprintln!("connect failed: {error}; retrying in {retry_in:?}");
          }
        }
      }
      _ = sync.tick() => writer.sync()?,
      () = &mut shutdown => break,
    }
  }
  if let Some(connection) = connected {
    writer.write(now_ms(), connection, Body::Close(SHUTDOWN.into()))?;
  }
  writer.sync()
}
