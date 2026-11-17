//! Certificates for TLS between a mesh's nodes: a private CA of the mesh's
//! own, which signs each node's certificate.
//!
//! The CA's key stays with whoever runs `cert`; it never goes to a node.
//! It's PKCS#8 PEM (ECDSA P-256 or P-384, Ed25519 or RSA), and may be
//! encrypted with a password (`ENCRYPTED PRIVATE KEY`, PBES2). Clients trust
//! the CA's certificate alone, so a node it signs needs no change on the
//! others. A CA certificate rebuilt from the same key (under [`CA_NAME`])
//! still vouches for the node certificates signed before.

use {
  ::pkcs8::{
    EncryptedPrivateKeyInfoRef, LineEnding, PrivateKeyInfoRef,
    der::{Document, SecretDocument},
  },
  ::rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
  },
  ::rustls::{
    RootCertStore, ServerConfig,
    client::{WebPkiServerVerifier, danger::ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{
      CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject,
    },
  },
  ::std::{io, sync::Arc},
  ::time::{Duration, OffsetDateTime},
  ::zeroize::Zeroizing,
};

/// The CA's subject name.
pub const CA_NAME: &str = "dgg-tools mesh CA";

/// How long a CA certificate lasts.
pub const CA_DAYS: i64 = 3650;

/// How long a node certificate lasts.
pub const NODE_DAYS: i64 = 730;

/// The PEM label of a PKCS#8 key.
const PLAIN: &str = "PRIVATE KEY";

/// The PEM label of a password-protected PKCS#8 key.
const ENCRYPTED: &str = "ENCRYPTED PRIVATE KEY";

/// Reads a PKCS#8 PEM private key, asking `password` for its password if
/// it's encrypted. Any other form is an error saying which are read.
pub fn read_key(
  pem: &str,
  password: impl FnOnce() -> io::Result<Zeroizing<String>>,
) -> Result<KeyPair, String> {
  let label = pem.lines().find_map(|line| {
    line
      .trim()
      .strip_prefix("-----BEGIN ")?
      .strip_suffix("-----")
  });
  let unreadable = |e: &dyn ::std::fmt::Display| format!("can't read it: {e}");
  match label {
    Some(PLAIN) => KeyPair::from_pem(pem).map_err(|e| unreadable(&e)),
    Some(ENCRYPTED) => {
      let (_, document) =
        Document::from_pem(pem).map_err(|e| unreadable(&e))?;
      let encrypted = EncryptedPrivateKeyInfoRef::try_from(document.as_bytes())
        .map_err(|e| unreadable(&e))?;
      let password =
        password().map_err(|e| format!("can't read the password: {e}"))?;
      let decrypted: SecretDocument = encrypted
        .decrypt(password.as_bytes())
        .map_err(|_| "wrong password, or a damaged key".to_string())?;
      KeyPair::try_from(decrypted.as_bytes()).map_err(|e| unreadable(&e))
    }
    Some(other) => Err(format!(
      "a `BEGIN {other}` key isn't read; give it as PKCS#8 (`BEGIN \
       {PLAIN}`, or `BEGIN {ENCRYPTED}` with a password)"
    )),
    None => Err(format!(
      "no PEM key in it (`BEGIN {PLAIN}` or `BEGIN {ENCRYPTED}`)"
    )),
  }
}

/// `key` as PKCS#8 PEM, encrypted with `password` (scrypt and AES-256-CBC)
/// if there is one.
pub fn key_pem(
  key: &KeyPair,
  password: Option<&str>,
) -> Result<Zeroizing<String>, String> {
  let Some(password) = password else {
    return Ok(Zeroizing::new(key.serialize_pem()));
  };
  let encrypt = || -> Result<_, ::pkcs8::Error> {
    PrivateKeyInfoRef::try_from(key.serialized_der())?
      .encrypt(password)?
      .to_pem(ENCRYPTED, LineEnding::LF)
      .map_err(Into::into)
  };
  encrypt().map_err(|e| format!("can't encrypt the key: {e}"))
}

/// From a day ago (for clocks behind this one's) to `days` from now.
fn validity(params: &mut CertificateParams, days: i64) {
  let now = OffsetDateTime::now_utc();
  params.not_before = now - Duration::days(1);
  params.not_after = now + Duration::days(days);
}

fn subject(common_name: &str) -> DistinguishedName {
  let mut name = DistinguishedName::new();
  name.push(DnType::CommonName, common_name);
  name
}

/// The CA's certificate, but for its validity: what signing needs.
fn ca_params() -> CertificateParams {
  let mut params = CertificateParams::default();
  params.distinguished_name = subject(CA_NAME);
  // It signs nodes' certificates, not other CAs'.
  params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
  params.key_usages = vec![
    KeyUsagePurpose::KeyCertSign,
    KeyUsagePurpose::CrlSign,
    KeyUsagePurpose::DigitalSignature,
  ];
  params
}

/// A CA certificate for `key`, for [`CA_DAYS`], as PEM.
pub fn ca(key: &KeyPair) -> Result<String, String> {
  let mut params = ca_params();
  validity(&mut params, CA_DAYS);
  let cert = params
    .self_signed(key)
    .map_err(|e| format!("can't make the CA certificate: {e}"))?;
  Ok(cert.pem())
}

/// A node's certificate and its key, as PEM.
pub struct Node {
  pub cert: String,
  pub key: Zeroizing<String>,
}

/// A certificate for node `name`, reached at `hosts` (IP addresses or DNS
/// names), with a new ECDSA P-256 key, for [`NODE_DAYS`]. It's signed with
/// `ca_key`, then checked against `ca_cert`, so a key that isn't that CA's
/// is caught here.
pub fn node(
  name: &str,
  hosts: &[String],
  ca_cert: &str,
  ca_key: &KeyPair,
) -> Result<Node, String> {
  if hosts.is_empty() {
    return Err("a node's certificate needs a host".into());
  }
  let mut params = CertificateParams::new(hosts.to_vec())
    .map_err(|e| format!("not a host: {e}"))?;
  params.distinguished_name = subject(name);
  params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
  params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
  params.use_authority_key_identifier_extension = true;
  validity(&mut params, NODE_DAYS);
  let key = KeyPair::generate()
    .map_err(|e| format!("can't make the node's key: {e}"))?;
  let cert = params
    .signed_by(&key, &Issuer::new(ca_params(), ca_key))
    .map_err(|e| format!("can't sign the node's certificate: {e}"))?;
  let node = Node {
    cert: cert.pem(),
    key: Zeroizing::new(key.serialize_pem()),
  };
  for host in hosts {
    verify(ca_cert, &node.cert, host).map_err(|e| {
      format!("the CA certificate doesn't vouch for what its key signs: {e}")
    })?;
  }
  Ok(node)
}

/// The crypto every TLS config here uses.
pub fn provider() -> Arc<CryptoProvider> {
  Arc::new(::rustls::crypto::ring::default_provider())
}

/// A root store holding just the certificates in `ca_cert` (PEM).
pub fn roots(ca_cert: &str) -> Result<RootCertStore, String> {
  let mut roots = RootCertStore::empty();
  for cert in CertificateDer::pem_slice_iter(ca_cert.as_bytes()) {
    let cert =
      cert.map_err(|e| format!("can't read the CA certificate: {e}"))?;
    roots
      .add(cert)
      .map_err(|e| format!("not a CA certificate: {e}"))?;
  }
  match roots.is_empty() {
    true => Err("no certificate in the CA certificate file".into()),
    false => Ok(roots),
  }
}

/// Whether `cert` (PEM) is one `ca_cert` vouches for, for `host`, as a
/// client trusting only that CA would check it.
pub fn verify(ca_cert: &str, cert: &str, host: &str) -> Result<(), String> {
  let verifier = WebPkiServerVerifier::builder_with_provider(
    roots(ca_cert)?.into(),
    provider(),
  )
  .build()
  .map_err(|e| e.to_string())?;
  let cert = CertificateDer::from_pem_slice(cert.as_bytes())
    .map_err(|e| format!("can't read the certificate: {e}"))?;
  let host = ServerName::try_from(host.to_string())
    .map_err(|_| format!("{host} isn't a host name or IP address"))?;
  verifier
    .verify_server_cert(&cert, &[], &host, &[], UnixTime::now())
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// `builder` trusting only the CA in `ca_cert` (PEM), and only over HTTPS.
pub fn trusting(
  builder: ::reqwest::ClientBuilder,
  ca_cert: &str,
) -> Result<::reqwest::ClientBuilder, String> {
  roots(ca_cert)?;
  let certs = ::reqwest::Certificate::from_pem_bundle(ca_cert.as_bytes())
    .map_err(|e| format!("can't read the CA certificate: {e}"))?;
  Ok(builder.tls_certs_only(certs).https_only(true))
}

/// A TLS server config serving `cert` (PEM: the node's certificate, which
/// may be followed by the CA's) with `key` (PEM, without a password), for
/// HTTP/2 and HTTP/1.1.
pub fn server_config(cert: &str, key: &str) -> Result<ServerConfig, String> {
  let certs = CertificateDer::pem_slice_iter(cert.as_bytes())
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("can't read the certificate: {e}"))?;
  if certs.is_empty() {
    return Err("no certificate in the certificate file".into());
  }
  let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|e| {
    format!("can't read the key (it's PEM, without a password): {e}")
  })?;
  let mut config = ServerConfig::builder_with_provider(provider())
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| format!("the key doesn't fit the certificate: {e}"))?;
  config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
  Ok(config)
}
