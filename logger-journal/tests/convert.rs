//! Converting records into the newest version, checked.

use ::logger_journal::{
  Header, Variant, Version,
  convert::{self, Error},
};

fn legacy() -> Header {
  Header::new(Variant::Dgg, Version { major: 0, minor: 1 })
}

#[test]
fn copies_what_it_can() {
  for record in [
    &b"5 a 1 frame MSG {}"[..],
    b"5 a 1 frame64 QQpC",
    // Doesn't decode, but reads the same: the same error.
    b"5 a 1 later x",
    b"5 a 1 frame64 !!",
  ] {
    let converted = convert::record(&legacy(), record).unwrap();
    assert_eq!(&*converted, record);
  }
  // Already the newest: as it is.
  let newest = convert::newest(Variant::Dgg);
  assert_eq!(newest.text(), "journal dgg.0.3\u{1e}\n");
  let record = b"5 a 1 frame A\nB";
  assert_eq!(&*convert::record(&newest, record).unwrap(), record);
}

#[test]
fn encodes_an_rs_again() {
  let converted = convert::record(&legacy(), b"5 a 1 frame A\x1eB").unwrap();
  assert_eq!(&*converted, b"5 a 1 frame64 QR5C");
  assert_eq!(
    convert::record(&legacy(), b"5 a 1 frame64 !\x1e"),
    Err(Error::HoldsRs)
  );
}
