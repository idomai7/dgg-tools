//! The mesh's CA and node certificates, and reading keys. The keys under
//! `fixtures/cert` were made with OpenSSL 3 for these tests alone; their
//! password is `fixture-password`.

use {
  ::logger::cert::{ca, key_pem, node, read_key, verify},
  ::rcgen::{KeyPair, PKCS_ECDSA_P384_SHA384, PKCS_ED25519, PKCS_RSA_SHA256},
  ::std::{cell::Cell, io},
  ::zeroize::Zeroizing,
};

const PASSWORD: &str = "fixture-password";

fn fixture(name: &str) -> String {
  let path =
    format!("{}/tests/fixtures/cert/{name}", env!("CARGO_MANIFEST_DIR"));
  ::std::fs::read_to_string(path).unwrap()
}

fn password(given: &str) -> impl FnOnce() -> io::Result<Zeroizing<String>> {
  move || Ok(Zeroizing::new(given.to_string()))
}

fn no_password() -> io::Result<Zeroizing<String>> {
  panic!("asked for a password")
}

fn hosts(hosts: &[&str]) -> Vec<String> {
  hosts.iter().map(|host| host.to_string()).collect()
}

#[test]
fn a_node_certificate_verifies_for_its_hosts_only() {
  let key = KeyPair::generate().unwrap();
  let ca_cert = ca(&key).unwrap();
  let node =
    node("do", &hosts(&["203.0.113.5", "do.example"]), &ca_cert, &key).unwrap();
  verify(&ca_cert, &node.cert, "203.0.113.5").unwrap();
  verify(&ca_cert, &node.cert, "do.example").unwrap();
  assert!(verify(&ca_cert, &node.cert, "198.51.100.7").is_err());
  assert!(verify(&ca_cert, &node.cert, "other.example").is_err());
  // Its own key is a plain one, for the node to serve with unattended.
  assert!(node.key.starts_with("-----BEGIN PRIVATE KEY-----"));
}

#[test]
fn another_cas_certificates_are_refused() {
  let key = KeyPair::generate().unwrap();
  let other = KeyPair::generate().unwrap();
  let ca_cert = ca(&key).unwrap();
  let other_cert = ca(&other).unwrap();
  let theirs =
    node("do", &hosts(&["203.0.113.5"]), &other_cert, &other).unwrap();
  assert!(verify(&ca_cert, &theirs.cert, "203.0.113.5").is_err());
  // Signing with a key that isn't the certificate's is caught.
  let mismatched = node("do", &hosts(&["203.0.113.5"]), &ca_cert, &other);
  assert!(mismatched.is_err());
}

#[test]
fn a_rebuilt_ca_still_vouches_for_its_nodes() {
  let key = KeyPair::generate().unwrap();
  let before = ca(&key).unwrap();
  let node = node("oracle", &hosts(&["198.51.100.7"]), &before, &key).unwrap();
  let after = ca(&key).unwrap();
  assert_ne!(before, after);
  verify(&after, &node.cert, "198.51.100.7").unwrap();
}

#[test]
fn encrypted_keys_round_trip() {
  for algorithm in [&PKCS_ED25519, &PKCS_ECDSA_P384_SHA384] {
    let key = KeyPair::generate_for(algorithm).unwrap();
    let pem = key_pem(&key, Some("a password")).unwrap();
    assert!(pem.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----"));
    let read = read_key(&pem, password("a password")).unwrap();
    assert_eq!(read.public_key_raw(), key.public_key_raw());
    // Without a password, as it was.
    let plain = key_pem(&key, None).unwrap();
    let read = read_key(&plain, no_password).unwrap();
    assert_eq!(read.public_key_raw(), key.public_key_raw());
  }
}

#[test]
fn reads_keys_openssl_wrote() {
  let asked = Cell::new(0);
  let counted = || {
    asked.set(asked.get() + 1);
    Ok(Zeroizing::new(PASSWORD.to_string()))
  };
  // PBKDF2 with SHA-256, and scrypt; both AES-256-CBC.
  let p384 = read_key(&fixture("p384-pbkdf2.pem"), counted).unwrap();
  assert_eq!(p384.algorithm(), &PKCS_ECDSA_P384_SHA384);
  let ed25519 = read_key(&fixture("ed25519-scrypt.pem"), counted).unwrap();
  assert_eq!(ed25519.algorithm(), &PKCS_ED25519);
  assert_eq!(asked.get(), 2);
  let rsa = read_key(&fixture("rsa.pem"), no_password).unwrap();
  assert_eq!(rsa.algorithm(), &PKCS_RSA_SHA256);
  // Any of them can be the CA's.
  for key in [p384, ed25519, rsa] {
    let ca_cert = ca(&key).unwrap();
    let node = node("pi", &hosts(&["192.0.2.9"]), &ca_cert, &key).unwrap();
    verify(&ca_cert, &node.cert, "192.0.2.9").unwrap();
  }
}

#[test]
fn a_wrong_password_is_refused() {
  let Err(e) = read_key(&fixture("p384-pbkdf2.pem"), password("not it")) else {
    panic!("read with the wrong password");
  };
  assert_eq!(e, "wrong password, or a damaged key");
}

#[test]
fn other_key_forms_are_errors() {
  let Err(e) = read_key(&fixture("p256-traditional.pem"), no_password) else {
    panic!("read an old-form key");
  };
  assert!(e.contains("`BEGIN EC PRIVATE KEY` key isn't read"), "{e}");
  assert!(e.contains("PKCS#8"), "{e}");
  let Err(e) = read_key("not a key", no_password) else {
    panic!("read nothing");
  };
  assert!(e.starts_with("no PEM key"), "{e}");
}
