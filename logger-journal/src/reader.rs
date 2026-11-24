//! Reads records from files of any readable version, live or archived.

use {
  crate::{
    layout::ARCHIVE_SUFFIX,
    record::{DecodeError, END, Header, Record, Variant, Version},
  },
  ::std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
  },
};

/// A record and where it sits in its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
  pub offset: u64,
  /// Offset just past this record: where to resume reading.
  pub next: u64,
  pub record: Record,
}

#[derive(Debug)]
pub enum ReadError {
  /// Reading stops after an I/O error.
  Io(io::Error),
  /// The file doesn't start with a header line.
  Header { found: String },
  /// The file's variant and major version have no decoder in this crate.
  Version(Variant, Version),
  /// A complete record that couldn't be decoded. Reading continues past it.
  Record { offset: u64, error: DecodeError },
}

impl From<io::Error> for ReadError {
  fn from(e: io::Error) -> Self {
    Self::Io(e)
  }
}

/// Reads complete records from an offset on, decoding them with the decoder
/// for the file's header variant and version.
///
/// A trailing record without its end ([`END`]) is still
/// being written (or was torn by a crash), so iteration stops before it;
/// read again from the last `next` to pick it up later.
///
/// Archives (`.journal.zst`) are read the same way, decompressed as they
/// go; their offsets are into the decompressed journal.
pub struct Reader {
  inner: Box<dyn BufRead + Send>,
  header: Option<Header>,
  offset: u64,
  record: Vec<u8>,
  done: bool,
}

impl Reader {
  /// Opens `path` and reads its header, then resumes at `offset` (0, or any
  /// offset inside the header, means the first record after it).
  ///
  /// A file whose header hasn't been completely written yet reads as empty.
  pub fn open(path: &Path, offset: u64) -> Result<Self, ReadError> {
    let file = File::open(path)?;
    let archived = path
      .file_name()
      .is_some_and(|name| name.to_string_lossy().ends_with(ARCHIVE_SUFFIX));
    let mut inner: Box<dyn BufRead + Send> = if archived {
      Box::new(BufReader::new(::zstd::Decoder::new(file)?))
    } else {
      Box::new(BufReader::new(file))
    };
    let mut header = Vec::new();
    let n = inner.read_until(b'\n', &mut header)? as u64;
    let Some(text) = header.strip_suffix(b"\n") else {
      return Ok(Self {
        inner,
        header: None,
        offset: 0,
        record: Vec::new(),
        done: true,
      });
    };
    let Some(header) = Header::parse(text) else {
      let found = String::from_utf8_lossy(text).into_owned();
      return Err(ReadError::Header { found });
    };
    if !header.readable() {
      return Err(ReadError::Version(header.variant, header.version));
    }
    let offset = offset.max(n);
    if archived {
      // A compressed stream can't seek: read up to the offset.
      io::copy(&mut (&mut inner).take(offset - n), &mut io::sink())?;
    } else {
      let mut file = File::open(path)?;
      file.seek(SeekFrom::Start(offset))?;
      inner = Box::new(BufReader::new(file));
    }
    Ok(Self {
      inner,
      header: Some(header),
      offset,
      record: Vec::new(),
      done: false,
    })
  }

  /// The file's header, or `None` if it isn't complete yet.
  pub fn header(&self) -> Option<&Header> {
    self.header.as_ref()
  }

  /// The file's version, or `None` if its header isn't complete yet.
  pub fn version(&self) -> Option<Version> {
    Some(self.header.as_ref()?.version)
  }

  /// Offset just past the last complete record read so far.
  pub fn offset(&self) -> u64 {
    self.offset
  }

  /// Counts the complete records left, up to `at_most`, without decoding
  /// them.
  pub fn count_records(mut self, at_most: u64) -> io::Result<u64> {
    let mut count = 0;
    while count < at_most {
      match self.next_record() {
        Some(Ok(_)) => count += 1,
        Some(Err(e)) => return Err(e),
        None => break,
      }
    }
    Ok(count)
  }

  /// Reads the next complete, non-empty record into `record`, without its
  /// end, and returns its offset.
  fn next_record(&mut self) -> Option<io::Result<u64>> {
    let end = END;
    while !self.done {
      self.record.clear();
      // A record may span lines: read them until its end.
      let complete = loop {
        match self.inner.read_until(b'\n', &mut self.record) {
          Ok(0) => break false,
          Ok(_) if self.record.ends_with(end) => break true,
          Ok(_) if self.record.last() == Some(&b'\n') => {}
          Ok(_) => break false,
          Err(e) => {
            self.done = true;
            return Some(Err(e));
          }
        }
      };
      if !complete {
        self.done = true;
        break;
      }
      let n = self.record.len();
      let offset = self.offset;
      self.offset += n as u64;
      self.record.truncate(n - end.len());
      if !self.record.is_empty() {
        return Some(Ok(offset));
      }
    }
    None
  }
}

impl Iterator for Reader {
  type Item = Result<Entry, ReadError>;

  fn next(&mut self) -> Option<Self::Item> {
    self.header.as_ref()?;
    let offset = match self.next_record()? {
      Ok(offset) => offset,
      Err(e) => return Some(Err(ReadError::Io(e))),
    };
    let header = self.header.as_ref()?;
    Some(match Record::decode(header, &self.record) {
      Ok(record) => Ok(Entry {
        offset,
        next: self.offset,
        record,
      }),
      Err(error) => Err(ReadError::Record { offset, error }),
    })
  }
}
