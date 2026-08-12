//! Safe wrappers over the system mbedTLS 3.6 shared library, purpose-built
//! for loft's DTLS/UDP listener (and its tests' client side). Dynamically
//! linked against Debian's apt-patched `libmbedtls.so.21` -- the same
//! zero-owned-crypto posture as the OpenSSL side -- with the only compiled
//! first-party C being a small non-crypto glue TU (csrc/glue.c) for
//! context allocation and header-inline setters. Design and decision
//! record: `docs/infra/coap-cid-design.md`.

mod config;
mod error;
mod ffi;
mod session;

pub use config::{CID_LEN, Config, PSK_SUITES, PskCallback, ResolvedPsk};
pub use error::MbedError;
pub use session::{
  CidStatus, HandshakeStatus, MbedIo, ReadStatus, RecvOutcome, SendOutcome, Session, TimerState,
};

/// The mbedTLS version actually loaded at runtime, e.g. "3.6.5" -- logged
/// at startup so the journal always names the linked library.
pub fn runtime_version() -> String {
  let mut buf = [0i8; 16];
  // SAFETY: `mbedtls_version_get_string` writes at most 9 bytes plus NUL
  // into the caller's buffer.
  unsafe { ffi::mbedtls_version_get_string(buf.as_mut_ptr()) };
  buf
    .iter()
    .take_while(|&&c| c != 0)
    .map(|&c| c as u8 as char)
    .collect()
}

/// Packed MBEDTLS_VERSION_NUMBER (e.g. 0x03060500) from the runtime
/// library.
pub fn runtime_version_number() -> u32 {
  // SAFETY: no arguments, returns a constant.
  unsafe { ffi::mbedtls_version_get_number() }
}
