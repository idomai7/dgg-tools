//! Converting records into the newest version of their variant's format,
//! checked: what the newest parser makes of the result must be what the
//! file's own parser made of the original.
//!
//! A record is copied byte for byte, unless the newest version can't hold
//! it as it is: a `journal 0.1` record holding an RS, which is decoded and
//! encoded again, its text as base64.

use {
  crate::record::{Framing, Header, RS, Record, Variant, kick, v0},
  ::std::{borrow::Cow, fmt},
};

/// Why a record couldn't be converted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
  /// It doesn't decode, so can't be encoded again, but holds an RS, so
  /// can't be copied either.
  HoldsRs,
  /// The newest parser makes something else of it than the file's did.
  Changed,
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.write_str(match self {
      Self::HoldsRs => "a record that doesn't decode holds an RS",
      Self::Changed => "a record would read differently once converted",
    })
  }
}

/// The header of `variant`'s newest version.
pub fn newest(variant: Variant) -> Header {
  match variant {
    Variant::Dgg => Header::new(Variant::Dgg, v0::VERSION),
    Variant::Kick => Header::new(Variant::Kick, kick::v0::VERSION),
  }
}

/// `record`, without its end, from a file with `header`, as the newest
/// version of its variant holds it, without its end.
///
/// Checked: the newest parser's decoding of the result is the same record
/// as the file's parser's decoding of the original, or for one that doesn't
/// decode, the same error, with the same bytes.
pub fn record<'a>(
  header: &Header,
  record: &'a [u8],
) -> Result<Cow<'a, [u8]>, Error> {
  let newest = newest(header.variant);
  let converted = if header.framing() == Framing::Lf && record.contains(&RS) {
    let Ok(Record::V0(decoded)) = Record::decode(header, record) else {
      return Err(Error::HoldsRs);
    };
    let mut encoded = Vec::new();
    decoded.encode(&mut encoded);
    encoded.truncate(encoded.len() - newest.framing().end().len());
    Cow::Owned(encoded)
  } else {
    Cow::Borrowed(record)
  };
  let same = match (
    Record::decode(header, record),
    Record::decode(&newest, &converted),
  ) {
    (Ok(old), Ok(new)) => old == new,
    (Err(old), Err(new)) => old == new && *converted == *record,
    _ => false,
  };
  if same { Ok(converted) } else { Err(Error::Changed) }
}
