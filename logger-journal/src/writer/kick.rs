//! Appends Kick records to one channel's live files, in the [`kick::v0`]
//! format.

use {
  super::{SYNC_INTERVAL, open_day},
  crate::record::{
    Header, Variant,
    kick::{self, v0::Body},
    v0::{valid_instance, valid_token},
  },
  ::common::time::utc_day,
  ::std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
    time::Instant,
  },
};

/// Appends one instance's records for one Kick channel to its live files,
/// rolling over at UTC midnight (by each record's `receive_time`), and
/// starting the day's next file, `<instance>.<n>.journal`, when an `open`
/// line is to a different endpoint than the file's.
///
/// Every line is written with a single `write` call, so a crash leaves at
/// most one torn line at the tail. Call [`Writer::sync_if_due`] on a timer to
/// bound how much a power loss can take.
pub struct Writer {
  root: PathBuf,
  instance: String,
  channel: String,
  /// The last `open` line's endpoint.
  endpoint: Option<String>,
  open: Option<Open>,
  dirty: bool,
  last_sync: Instant,
}

/// The file being written.
struct Open {
  day: String,
  endpoint: Option<String>,
  file: File,
}

impl Writer {
  /// A writer for `instance` under `root`, the channel's journal root (see
  /// [`channel_root`](crate::layout::channel_root)), with `channel` its
  /// slug. Nothing is opened until the first [`Writer::write`].
  ///
  /// # Panics
  ///
  /// If `instance` isn't [`valid_instance`] or `channel` isn't a
  /// [`valid_token`].
  pub fn new(
    root: impl Into<PathBuf>,
    instance: impl Into<String>,
    channel: impl Into<String>,
  ) -> Self {
    let (instance, channel) = (instance.into(), channel.into());
    assert!(
      valid_instance(&instance),
      "instance id must be [A-Za-z0-9._-]+: {instance:?}"
    );
    assert!(valid_token(&channel), "not a valid channel: {channel:?}");
    Self {
      root: root.into(),
      instance,
      channel,
      endpoint: None,
      open: None,
      dirty: false,
      last_sync: Instant::now(),
    }
  }

  /// The instance id written on every line.
  pub fn instance(&self) -> &str {
    &self.instance
  }

  /// Appends one line to the live file for `receive_time`'s UTC day and the
  /// endpoint, opening (and on a change, syncing and switching) files as
  /// needed. The line reaches the OS before this returns, but isn't
  /// fsynced; see [`Writer::sync`].
  ///
  /// An `open` line to an endpoint that isn't a [`valid_token`] fails, as
  /// the header couldn't hold it.
  pub fn write(
    &mut self,
    receive_time: i64,
    connection: u64,
    body: Body,
  ) -> io::Result<()> {
    if let Body::Open(endpoint) = &body {
      if !valid_token(endpoint) {
        let error = format!("not a valid endpoint: {endpoint:?}");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, error));
      }
      self.endpoint = Some(endpoint.clone());
    }
    let day = utc_day(receive_time);
    if self
      .open
      .as_ref()
      .is_none_or(|open| open.day != day || open.endpoint != self.endpoint)
    {
      self.sync()?;
      let file = self.open_day(&day)?;
      self.open = Some(Open {
        day,
        endpoint: self.endpoint.clone(),
        file,
      });
    }
    let record = kick::v0::Record {
      receive_time,
      instance: self.instance.clone(),
      connection,
      body,
    };
    let mut line = Vec::new();
    record.encode(&mut line);
    let open = self.open.as_mut().unwrap();
    open.file.write_all(&line)?;
    self.dirty = true;
    Ok(())
  }

  /// Fsyncs the open file if anything was written since the last sync.
  pub fn sync(&mut self) -> io::Result<()> {
    if let Some(open) = &self.open
      && self.dirty
    {
      open.file.sync_data()?;
    }
    self.dirty = false;
    self.last_sync = Instant::now();
    Ok(())
  }

  /// [`Writer::sync`]s if [`SYNC_INTERVAL`] has passed since the last sync.
  pub fn sync_if_due(&mut self) -> io::Result<()> {
    if self.last_sync.elapsed() >= SYNC_INTERVAL {
      self.sync()?;
    }
    Ok(())
  }

  /// Opens the day's last file if its header is this one's, endpoint
  /// included, or starts the next.
  fn open_day(&self, day: &str) -> io::Result<File> {
    open_day(&self.root, day, &self.instance, &self.header())
  }

  /// The header for a new file.
  fn header(&self) -> Header {
    let header = Header::new(Variant::Kick, kick::v0::VERSION);
    let header = match &self.endpoint {
      Some(endpoint) => header.with("endpoint", endpoint),
      None => header,
    };
    header.with("channel", &self.channel)
  }
}

impl Drop for Writer {
  fn drop(&mut self) {
    let _ = self.sync();
  }
}
