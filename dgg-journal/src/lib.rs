//! The logger's journal: append-only JSON-lines files that are the source of
//! truth for everything captured from dgg chat.
//!
//! Live files live at `<root>/live/YYYY-MM-DD/<instance>.jsonl`, one per
//! capture instance per UTC day, so overlapping instances never share a file.

use {
  ::serde::{Deserialize, Serialize},
  ::std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
  },
};

/// How often [`Writer::sync_if_due`] fsyncs.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// One journal line: shared fields plus a [`Body`] whose variant name is the
/// JSON key, e.g. `{"recv":…,"inst":"…","conn":3,"frame":"MSG {…}"}`.
///
/// Unknown extra keys are ignored; a line with no known body key fails to
/// parse, so a future kind is never mistaken for a frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
  /// Local receive time, unix ms.
  pub recv: i64,
  /// Capture instance that wrote the line.
  pub inst: String,
  /// Connection number within the instance, counting from 1.
  pub conn: u64,
  #[serde(flatten)]
  pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Body {
  /// A dgg text frame exactly as received (`EVENT payload`).
  Frame(String),
  /// A binary frame, base64 in the journal. dgg doesn't send these today;
  /// they're kept so a protocol change can't silently lose data.
  Binary(#[serde(with = "base64")] Vec<u8>),
  /// Connection opened, with the URL it connected to.
  Open(String),
  /// Connection dropped, with why.
  Close(String),
}

mod base64 {
  use {
    ::base64::{Engine, engine::general_purpose::STANDARD},
    ::serde::{Deserialize, Deserializer, Serializer, de::Error},
  };

  pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&STANDARD.encode(v))
  }

  pub fn deserialize<'de, D: Deserializer<'de>>(
    d: D,
  ) -> Result<Vec<u8>, D::Error> {
    // Owned: `flatten` buffers values, so borrowing would fail.
    let s = String::deserialize(d)?;
    STANDARD.decode(s).map_err(D::Error::custom)
  }
}

/// `YYYY-MM-DD` of a unix ms timestamp, in UTC.
pub fn utc_day(ms: i64) -> String {
  // Howard Hinnant's civil_from_days.
  let z = ms.div_euclid(86_400_000) + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z - era * 146_097;
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = doy - (153 * mp + 2) / 5 + 1;
  let m = if mp < 10 { mp + 3 } else { mp - 9 };
  let y = yoe + era * 400 + i64::from(m <= 2);
  format!("{y:04}-{m:02}-{d:02}")
}

pub fn live_dir(root: &Path) -> PathBuf {
  root.join("live")
}

/// Every live journal file, sorted by day then instance.
pub fn live_files(root: &Path) -> io::Result<Vec<PathBuf>> {
  let mut files = Vec::new();
  let days = match fs::read_dir(live_dir(root)) {
    Ok(days) => days,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(files),
    Err(e) => return Err(e),
  };
  for day in days {
    let day = day?;
    if !day.file_type()?.is_dir() {
      continue;
    }
    for file in fs::read_dir(day.path())? {
      let path = file?.path();
      if path.extension().is_some_and(|e| e == "jsonl") {
        files.push(path);
      }
    }
  }
  files.sort();
  Ok(files)
}

/// Appends one instance's records to its live files, rolling over at UTC
/// midnight (by each record's `recv`).
///
/// Every line is written with a single `write` call, so a crash leaves at
/// most one torn line at the tail. Call [`Writer::sync_if_due`] on a timer to
/// bound how much a power loss can take.
pub struct Writer {
  root: PathBuf,
  inst: String,
  open: Option<(String, File)>,
  dirty: bool,
  last_sync: Instant,
}

impl Writer {
  pub fn new(root: impl Into<PathBuf>, inst: impl Into<String>) -> Self {
    let inst = inst.into();
    assert!(
      !inst.is_empty()
        && inst
          .bytes()
          .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
      "instance id must be [A-Za-z0-9._-]+: {inst:?}"
    );
    Self {
      root: root.into(),
      inst,
      open: None,
      dirty: false,
      last_sync: Instant::now(),
    }
  }

  pub fn inst(&self) -> &str {
    &self.inst
  }

  pub fn write(&mut self, rec: &Record) -> io::Result<()> {
    let day = utc_day(rec.recv);
    if self.open.as_ref().is_none_or(|(d, _)| *d != day) {
      self.sync()?;
      self.open = Some((day.clone(), self.open_day(&day)?));
    }
    let mut line = ::serde_json::to_vec(rec)?;
    line.push(b'\n');
    let (_, file) = self.open.as_mut().unwrap();
    file.write_all(&line)?;
    self.dirty = true;
    Ok(())
  }

  pub fn sync(&mut self) -> io::Result<()> {
    if let Some((_, file)) = &self.open
      && self.dirty
    {
      file.sync_data()?;
    }
    self.dirty = false;
    self.last_sync = Instant::now();
    Ok(())
  }

  pub fn sync_if_due(&mut self) -> io::Result<()> {
    if self.last_sync.elapsed() >= SYNC_INTERVAL {
      self.sync()?;
    }
    Ok(())
  }

  fn open_day(&self, day: &str) -> io::Result<File> {
    let dir = live_dir(&self.root).join(day);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.jsonl", self.inst));
    let mut file = OpenOptions::new()
      .create(true)
      .read(true)
      .append(true)
      .open(path)?;
    // Isolate a torn line left by a crash, so the next record starts clean.
    if file.metadata()?.len() > 0 {
      let mut last = [0];
      file.seek(SeekFrom::End(-1))?;
      file.read_exact(&mut last)?;
      if last[0] != b'\n' {
        file.write_all(b"\n")?;
      }
    }
    Ok(file)
  }
}

impl Drop for Writer {
  fn drop(&mut self) {
    let _ = self.sync();
  }
}

/// A record and where it sits in its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
  pub offset: u64,
  /// Offset just past this line: where to resume reading.
  pub next: u64,
  pub record: Record,
}

#[derive(Debug)]
pub enum ReadError {
  Io(io::Error),
  /// A complete line that isn't a valid record (e.g. a torn crash line).
  /// Reading continues past it.
  Bad {
    offset: u64,
    error: ::serde_json::Error,
  },
}

/// Reads complete lines from `offset` on. A trailing line without its
/// newline is still being written (or was torn by a crash), so iteration
/// stops before it; read again from the last `next` to pick it up later.
pub struct Reader {
  inner: BufReader<File>,
  offset: u64,
  line: Vec<u8>,
  done: bool,
}

impl Reader {
  pub fn open(path: &Path, offset: u64) -> io::Result<Self> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    Ok(Self {
      inner: BufReader::new(file),
      offset,
      line: Vec::new(),
      done: false,
    })
  }

  /// Offset just past the last complete line read so far.
  pub fn offset(&self) -> u64 {
    self.offset
  }
}

impl Iterator for Reader {
  type Item = Result<Entry, ReadError>;

  fn next(&mut self) -> Option<Self::Item> {
    while !self.done {
      self.line.clear();
      let n = match self.inner.read_until(b'\n', &mut self.line) {
        Ok(n) => n,
        Err(e) => {
          self.done = true;
          return Some(Err(ReadError::Io(e)));
        }
      };
      if n == 0 || self.line.last() != Some(&b'\n') {
        self.done = true;
        break;
      }
      let offset = self.offset;
      self.offset += n as u64;
      let text = &self.line[..n - 1];
      if text.is_empty() {
        continue;
      }
      return Some(match ::serde_json::from_slice(text) {
        Ok(record) => Ok(Entry {
          offset,
          next: self.offset,
          record,
        }),
        Err(error) => Err(ReadError::Bad { offset, error }),
      });
    }
    None
  }
}

#[cfg(test)]
mod tests {
  use {
    super::*,
    ::std::sync::atomic::{AtomicU32, Ordering},
  };

  fn temp_root() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = ::std::env::temp_dir()
      .join(format!("dgg-journal-test-{}-{n}", ::std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
  }

  fn rec(recv: i64, frame: &str) -> Record {
    Record {
      recv,
      inst: "a".into(),
      conn: 1,
      body: Body::Frame(frame.into()),
    }
  }

  fn frame(e: &Result<Entry, ReadError>) -> &str {
    match &e.as_ref().unwrap().record.body {
      Body::Frame(f) => f,
      b => panic!("not a frame: {b:?}"),
    }
  }

  #[test]
  fn line_format() {
    let line = |body| {
      let r = Record {
        recv: 5,
        inst: "a".into(),
        conn: 2,
        body,
      };
      let s = ::serde_json::to_string(&r).unwrap();
      assert_eq!(::serde_json::from_str::<Record>(&s).unwrap(), r);
      s
    };
    assert_eq!(
      line(Body::Frame("MSG {}".into())),
      r#"{"recv":5,"inst":"a","conn":2,"frame":"MSG {}"}"#
    );
    assert_eq!(
      line(Body::Binary(vec![0, 255])),
      r#"{"recv":5,"inst":"a","conn":2,"binary":"AP8="}"#
    );
    assert_eq!(
      line(Body::Open("wss://x".into())),
      r#"{"recv":5,"inst":"a","conn":2,"open":"wss://x"}"#
    );
    assert_eq!(
      line(Body::Close("idle".into())),
      r#"{"recv":5,"inst":"a","conn":2,"close":"idle"}"#
    );

    let parse = ::serde_json::from_str::<Record>;
    let r = parse(r#"{"recv":5,"inst":"a","conn":2,"src":"x","frame":"Q"}"#);
    assert_eq!(r.unwrap().body, Body::Frame("Q".into()));
    assert!(parse(r#"{"recv":5,"inst":"a","conn":2,"later":"x"}"#).is_err());
    assert!(parse(r#"{"recv":5,"inst":"a","conn":2}"#).is_err());
  }

  #[test]
  fn utc_days() {
    assert_eq!(utc_day(0), "1970-01-01");
    assert_eq!(utc_day(-1), "1969-12-31");
    assert_eq!(utc_day(1_709_164_800_000), "2024-02-29");
    assert_eq!(utc_day(1_709_251_199_999), "2024-02-29");
    assert_eq!(utc_day(1_794_128_504_611), "2026-11-08");
  }

  #[test]
  fn write_read_and_rollover() {
    let root = temp_root();
    let mut w = Writer::new(&root, "a");
    let recs = [
      rec(1_709_251_199_998, "MSG {\"n\":1}"),
      rec(1_709_251_199_999, "MSG {\"n\":2}"),
      rec(1_709_251_200_000, "MSG {\"n\":3}"),
    ];
    for r in &recs {
      w.write(r).unwrap();
    }
    drop(w);

    let files = live_files(&root).unwrap();
    let names: Vec<_> = files
      .iter()
      .map(|p| p.strip_prefix(live_dir(&root)).unwrap().to_owned())
      .collect();
    assert_eq!(
      names,
      [
        Path::new("2024-02-29").join("a.jsonl"),
        Path::new("2024-03-01").join("a.jsonl"),
      ]
    );
    let read: Vec<_> = files
      .iter()
      .flat_map(|f| Reader::open(f, 0).unwrap())
      .map(|e| e.unwrap().record)
      .collect();
    assert_eq!(read, recs);
    fs::remove_dir_all(&root).unwrap();
  }

  #[test]
  fn partial_tail_is_left_for_later() {
    let root = temp_root();
    let mut w = Writer::new(&root, "a");
    w.write(&rec(0, "MSG 1")).unwrap();
    w.write(&rec(0, "MSG 2")).unwrap();
    drop(w);
    let path = &live_files(&root).unwrap()[0];
    let full = fs::read(path).unwrap();
    let mut third = ::serde_json::to_vec(&rec(0, "MSG 3")).unwrap();
    let (head, tail) = third.split_at_mut(10);
    OpenOptions::new()
      .append(true)
      .open(path)
      .unwrap()
      .write_all(head)
      .unwrap();

    let mut r = Reader::open(path, 0).unwrap();
    assert_eq!(r.by_ref().count(), 2);
    assert_eq!(r.offset(), full.len() as u64);

    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(tail).unwrap();
    f.write_all(b"\n").unwrap();
    let rest: Vec<_> = Reader::open(path, r.offset()).unwrap().collect();
    assert_eq!(rest.len(), 1);
    assert_eq!(frame(&rest[0]), "MSG 3");
    fs::remove_dir_all(&root).unwrap();
  }

  #[test]
  fn torn_line_is_isolated_on_reopen() {
    let root = temp_root();
    let mut w = Writer::new(&root, "a");
    w.write(&rec(0, "MSG 1")).unwrap();
    drop(w);
    let path = live_files(&root).unwrap()[0].clone();
    OpenOptions::new()
      .append(true)
      .open(&path)
      .unwrap()
      .write_all(b"{\"recv\":0,\"in")
      .unwrap();

    let mut w = Writer::new(&root, "a");
    w.write(&rec(0, "MSG 2")).unwrap();
    drop(w);
    let read: Vec<_> = Reader::open(&path, 0).unwrap().collect();
    assert_eq!(read.len(), 3);
    assert_eq!(frame(&read[0]), "MSG 1");
    assert!(matches!(read[1], Err(ReadError::Bad { .. })));
    assert_eq!(frame(&read[2]), "MSG 2");
    fs::remove_dir_all(&root).unwrap();
  }
}
