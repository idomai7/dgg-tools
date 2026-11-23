//! Randomness, of two kinds.
//!
//! - From the OS, for what has to be unguessable: [`fill`], [`token`] and
//!   [`uuid`].
//! - From a fast generator (fastrand's wyrand), for what only has to be
//!   spread out: [`fraction`], and [`backoff`]'s jitter.

use {
  ::base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
  ::std::time::Duration,
};

/// Fills `bytes` from the OS.
pub fn fill(bytes: &mut [u8]) {
  ::getrandom::fill(bytes).expect("the OS has randomness");
}

/// `bytes` random bytes from the OS, in unpadded base64url: for secrets in
/// URLs, such as OAuth's `state` and PKCE's verifier.
pub fn token(bytes: usize) -> String {
  let mut random = vec![0; bytes];
  fill(&mut random);
  URL_SAFE_NO_PAD.encode(random)
}

/// A random (version 4) uuid from the OS, in lowercase hex with dashes.
pub fn uuid() -> String {
  let mut bytes = [0; 16];
  fill(&mut bytes);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
  format!(
    "{}-{}-{}-{}-{}",
    &hex[..8],
    &hex[8..12],
    &hex[12..16],
    &hex[16..20],
    &hex[20..]
  )
}

/// A number in `[0, 1)`, from the fast generator: not for secrets.
pub fn fraction() -> f64 {
  ::fastrand::f64()
}

/// The `step`th wait between retries, from 0: it doubles from `min` up to
/// `max`, with full jitter, so clients retrying together spread out.
pub fn backoff(step: u32, min: Duration, max: Duration) -> Duration {
  let ceiling = min.saturating_mul(1 << step.min(31)).min(max);
  (min + ceiling.mul_f64(fraction())).min(max)
}
