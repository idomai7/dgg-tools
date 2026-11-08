//! Reads records from files of any readable version.

use {
  crate::record::{DecodeError, Record, Version},
  ::std::{
    fs::File,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::Path,
  },
};

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
  /// The file doesn't start with a header line.
  Header { found: String },
  /// The file's major version has no decoder in this crate.
  Version(Version),
  /// A complete line that couldn't be decoded. Reading continues past it.
  Line { offset: u64, error: DecodeError },
}

impl From<io::Error> for ReadError {
  fn from(e: io::Error) -> Self {
    Self::Io(e)
  }
}

/// Reads complete lines from an offset on, decoding them with the decoder
/// for the file's header version.
///
/// A trailing line without its newline is still being written (or was torn
/// by a crash), so iteration stops before it; read again from the last
/// `next` to pick it up later.
pub struct Reader {
  inner: BufReader<File>,
  version: Option<Version>,
  offset: u64,
  line: Vec<u8>,
  done: bool,
}

impl Reader {
  /// Opens `path` and reads its header, then resumes at `offset` (0, or any
  /// offset inside the header, means the first line after it).
  ///
  /// A file whose header hasn't been completely written yet reads as empty.
  pub fn open(path: &Path, offset: u64) -> Result<Self, ReadError> {
    let mut inner = BufReader::new(File::open(path)?);
    let mut header = Vec::new();
    let n = inner.read_until(b'\n', &mut header)? as u64;
    let mut reader = Self {
      inner,
      version: None,
      offset: 0,
      line: Vec::new(),
      done: true,
    };
    let Some(text) = header.strip_suffix(b"\n") else {
      return Ok(reader);
    };
    let Some(version) = Version::parse_header(text) else {
      let found = String::from_utf8_lossy(text).into_owned();
      return Err(ReadError::Header { found });
    };
    if !version.readable() {
      return Err(ReadError::Version(version));
    }
    reader.version = Some(version);
    reader.offset = offset.max(n);
    reader.inner.seek(SeekFrom::Start(reader.offset))?;
    reader.done = false;
    Ok(reader)
  }

  /// The file's version, or `None` if its header isn't complete yet.
  pub fn version(&self) -> Option<Version> {
    self.version
  }

  /// Offset just past the last complete line read so far.
  pub fn offset(&self) -> u64 {
    self.offset
  }
}

impl Iterator for Reader {
  type Item = Result<Entry, ReadError>;

  fn next(&mut self) -> Option<Self::Item> {
    let version = self.version?;
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
      return Some(match Record::decode(version, text) {
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
