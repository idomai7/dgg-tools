//! The logger's journal: append-only line files that are the source of truth
//! for everything captured from dgg chat.
//!
//! Live files live at `<root>/live/YYYY-MM-DD/<instance>.journal`, one per
//! capture instance per UTC day, so overlapping instances never share a file.
//!
//! # Format
//!
//! The first line of every file is [`HEADER`]. Each later line is
//!
//! ```text
//! <recv> <inst> <conn> <kind> <data>
//! ```
//!
//! separated by single spaces. `recv` (local receive time, unix ms) and
//! `conn` are decimal digits, `inst` is `[A-Za-z0-9._-]+`, and `kind` is
//! `[a-z0-9]+`. `data` is the rest of the line, verbatim; the space before it
//! is omitted when it's empty. Kinds:
//!
//! - `frame`: a dgg text frame exactly as received (`EVENT payload`).
//! - `binary`: a binary frame, base64.
//! - `open`: a connection opened; data is the URL.
//! - `close`: a connection dropped; data is why.
//!
//! Text containing a CR or LF can't sit on one line, so it's written with the
//! kind suffixed `64` and the data base64 (`frame64`, `open64`, `close64`).

use {
  ::base64::{Engine, engine::general_purpose::STANDARD as BASE64},
  ::std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
  },
};

/// First line of every journal file: the format name and version.
pub const HEADER: &str = "dgg-journal 1";

/// File extension of live journal files.
pub const EXTENSION: &str = "journal";

/// How often [`Writer::sync_if_due`] fsyncs.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// One journal line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
  /// Local receive time, unix ms.
  pub recv: i64,
  /// Capture instance that wrote the line.
  pub inst: String,
  /// Connection number within the instance, counting from 1.
  pub conn: u64,
  pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
  /// A dgg text frame exactly as received (`EVENT payload`).
  Frame(String),
  /// A binary frame. dgg doesn't send these today; they're kept so a
  /// protocol change can't silently lose data.
  Binary(Vec<u8>),
  /// Connection opened, with the URL it connected to.
  Open(String),
  /// Connection dropped, with why.
  Close(String),
}

/// Why a complete line couldn't be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
  /// Not a well-formed line, e.g. one torn by a crash.
  Bad(&'static str),
  /// Well-formed, but a kind this version doesn't know (written by a newer
  /// version, say).
  Unknown(String),
}

pub fn valid_inst(inst: &str) -> bool {
  !inst.is_empty()
    && inst
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn digits(s: &str) -> bool {
  !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn valid_kind(s: &str) -> bool {
  !s.is_empty()
    && s
      .bytes()
      .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

impl Record {
  /// Appends this record's line, newline included, to `out`.
  pub fn encode(&self, out: &mut Vec<u8>) {
    let (kind, data) = match &self.body {
      Body::Frame(t) => ("frame", t.as_bytes()),
      Body::Open(t) => ("open", t.as_bytes()),
      Body::Close(t) => ("close", t.as_bytes()),
      Body::Binary(b) => ("binary", b.as_slice()),
    };
    let binary = matches!(self.body, Body::Binary(_));
    let text64 = !binary && data.iter().any(|&b| b == b'\n' || b == b'\r');
    let suffix = if text64 { "64" } else { "" };
    let header =
      format!("{} {} {} {kind}{suffix}", self.recv, self.inst, self.conn);
    out.extend_from_slice(header.as_bytes());
    if !data.is_empty() {
      out.push(b' ');
      if binary || text64 {
        out.extend_from_slice(BASE64.encode(data).as_bytes());
      } else {
        out.extend_from_slice(data);
      }
    }
    out.push(b'\n');
  }

  /// Decodes one line, without its newline.
  pub fn decode(line: &[u8]) -> Result<Self, DecodeError> {
    use DecodeError::Bad;
    let line = ::std::str::from_utf8(line).map_err(|_| Bad("not utf-8"))?;
    let mut parts = line.splitn(5, ' ');
    let mut field = || parts.next().ok_or(Bad("too few fields"));
    let (recv, inst, conn, kind) = (field()?, field()?, field()?, field()?);
    let data = parts.next().unwrap_or("");
    if !digits(recv) || !digits(conn) {
      return Err(Bad("recv/conn not decimal"));
    }
    if !valid_inst(inst) {
      return Err(Bad("bad inst"));
    }
    if !valid_kind(kind) {
      return Err(Bad("bad kind"));
    }
    let recv = recv.parse().map_err(|_| Bad("recv out of range"))?;
    let conn = conn.parse().map_err(|_| Bad("conn out of range"))?;
    let unbase64 = || BASE64.decode(data).map_err(|_| Bad("bad base64"));
    let text64 =
      || String::from_utf8(unbase64()?).map_err(|_| Bad("base64 not utf-8"));
    let body = match kind {
      "frame" => Body::Frame(data.into()),
      "open" => Body::Open(data.into()),
      "close" => Body::Close(data.into()),
      "frame64" => Body::Frame(text64()?),
      "open64" => Body::Open(text64()?),
      "close64" => Body::Close(text64()?),
      "binary" => Body::Binary(unbase64()?),
      _ => return Err(DecodeError::Unknown(kind.into())),
    };
    Ok(Self {
      recv,
      inst: inst.into(),
      conn,
      body,
    })
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
      if path.extension().is_some_and(|e| e == EXTENSION) {
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
      valid_inst(&inst),
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

  pub fn write(&mut self, recv: i64, conn: u64, body: Body) -> io::Result<()> {
    let day = utc_day(recv);
    if self.open.as_ref().is_none_or(|(d, _)| *d != day) {
      self.sync()?;
      self.open = Some((day.clone(), self.open_day(&day)?));
    }
    let rec = Record {
      recv,
      inst: self.inst.clone(),
      conn,
      body,
    };
    let mut line = Vec::new();
    rec.encode(&mut line);
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
    let path = dir.join(format!("{}.{EXTENSION}", self.inst));
    let mut file = OpenOptions::new()
      .create(true)
      .read(true)
      .append(true)
      .open(path)?;
    if file.metadata()?.len() == 0 {
      file.write_all(format!("{HEADER}\n").as_bytes())?;
    } else {
      // Isolate a torn line left by a crash, so the next record starts clean.
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
  /// Reading stops after an I/O error.
  Io(io::Error),
  /// The file doesn't start with [`HEADER`]. Reading stops.
  Header { found: String },
  /// A complete line that couldn't be decoded. Reading continues past it.
  Line { offset: u64, error: DecodeError },
}

/// Reads complete lines from `offset` on, checking [`HEADER`] when starting
/// at 0. A trailing line without its newline is still being written (or was
/// torn by a crash), so iteration stops before it; read again from the last
/// `next` to pick it up later.
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
      if offset == 0 {
        if text != HEADER.as_bytes() {
          self.done = true;
          let found = String::from_utf8_lossy(text).into_owned();
          return Some(Err(ReadError::Header { found }));
        }
        continue;
      }
      if text.is_empty() {
        continue;
      }
      return Some(match Record::decode(text) {
        Ok(record) => Ok(Entry {
          offset,
          next: self.offset,
          record,
        }),
        Err(error) => Err(ReadError::Line { offset, error }),
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

  fn write(w: &mut Writer, r: &Record) {
    w.write(r.recv, r.conn, r.body.clone()).unwrap();
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
      let mut out = Vec::new();
      r.encode(&mut out);
      let s = String::from_utf8(out).unwrap();
      let s = s.strip_suffix('\n').unwrap().to_owned();
      assert_eq!(Record::decode(s.as_bytes()), Ok(r));
      s
    };
    assert_eq!(
      line(Body::Frame(r#"MSG {"data":"hi \"x\""}"#.into())),
      r#"5 a 2 frame MSG {"data":"hi \"x\""}"#
    );
    assert_eq!(line(Body::Frame("A\nB".into())), "5 a 2 frame64 QQpC");
    assert_eq!(line(Body::Binary(vec![0, 255])), "5 a 2 binary AP8=");
    assert_eq!(line(Body::Binary(vec![])), "5 a 2 binary");
    assert_eq!(line(Body::Open("wss://x".into())), "5 a 2 open wss://x");
    assert_eq!(
      line(Body::Close("idle timeout".into())),
      "5 a 2 close idle timeout"
    );
    assert_eq!(line(Body::Close("".into())), "5 a 2 close");
    assert_eq!(line(Body::Close("a\r".into())), "5 a 2 close64 YQ0=");

    let decode = |s: &str| Record::decode(s.as_bytes());
    assert_eq!(
      decode("5 a 2 later x"),
      Err(DecodeError::Unknown("later".into()))
    );
    for bad in [
      "",
      "5 a 2",
      "+5 a 2 frame x",
      "5 a -2 frame x",
      "5 a/b 2 frame x",
      "5 a 2 Frame x",
      "5 a 2 binary !!",
      "5  a 2 frame x",
    ] {
      assert!(matches!(decode(bad), Err(DecodeError::Bad(_))), "{bad:?}");
    }
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
      write(&mut w, r);
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
        Path::new("2024-02-29").join("a.journal"),
        Path::new("2024-03-01").join("a.journal"),
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
  fn header_is_checked() {
    let root = temp_root();
    let mut w = Writer::new(&root, "a");
    write(&mut w, &rec(0, "MSG 1"));
    drop(w);
    let path = live_files(&root).unwrap()[0].clone();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text, "dgg-journal 1\n0 a 1 frame MSG 1\n");

    fs::write(&path, text.replacen("journal 1", "journal 2", 1)).unwrap();
    let read: Vec<_> = Reader::open(&path, 0).unwrap().collect();
    assert!(matches!(
      &read[..],
      [Err(ReadError::Header { found })] if found == "dgg-journal 2"
    ));
    fs::remove_dir_all(&root).unwrap();
  }

  #[test]
  fn partial_tail_is_left_for_later() {
    let root = temp_root();
    let mut w = Writer::new(&root, "a");
    write(&mut w, &rec(0, "MSG 1"));
    write(&mut w, &rec(0, "MSG 2"));
    drop(w);
    let path = &live_files(&root).unwrap()[0];
    let full = fs::read(path).unwrap();
    let mut third = Vec::new();
    rec(0, "MSG 3").encode(&mut third);
    let (head, tail) = third.split_at(10);
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
    let rest: Vec<_> = Reader::open(path, r.offset()).unwrap().collect();
    assert_eq!(rest.len(), 1);
    assert_eq!(frame(&rest[0]), "MSG 3");
    fs::remove_dir_all(&root).unwrap();
  }

  #[test]
  fn torn_line_is_isolated_on_reopen() {
    let root = temp_root();
    let mut w = Writer::new(&root, "a");
    write(&mut w, &rec(0, "MSG 1"));
    drop(w);
    let path = live_files(&root).unwrap()[0].clone();
    OpenOptions::new()
      .append(true)
      .open(&path)
      .unwrap()
      .write_all(b"0 a 1 fra")
      .unwrap();

    let mut w = Writer::new(&root, "a");
    write(&mut w, &rec(0, "MSG 2"));
    drop(w);
    let read: Vec<_> = Reader::open(&path, 0).unwrap().collect();
    assert_eq!(read.len(), 3);
    assert_eq!(frame(&read[0]), "MSG 1");
    assert!(matches!(
      read[1],
      Err(ReadError::Line {
        error: DecodeError::Unknown(_),
        ..
      })
    ));
    assert_eq!(frame(&read[2]), "MSG 2");
    fs::remove_dir_all(&root).unwrap();
  }
}
