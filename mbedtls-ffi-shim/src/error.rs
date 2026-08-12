//! mbedTLS error codes this crate matches on, plus a displayable error
//! wrapper. The numeric values are hardcoded from the 3.6 headers rather
//! than read through FFI so they can be used in `match` arms; the
//! `constants_match_headers` unit test proves every one of them against
//! the real headers (via the glue TU's `shim_const`), so silent drift on a
//! library upgrade fails the test suite instead of miscompiling.

use std::fmt;
use std::os::raw::c_int;

pub(crate) mod codes {
  use std::os::raw::c_int;

  pub const ERR_SSL_WANT_READ: c_int = -0x6900;
  pub const ERR_SSL_WANT_WRITE: c_int = -0x6880;
  pub const ERR_SSL_TIMEOUT: c_int = -0x6800;
  pub const ERR_SSL_HELLO_VERIFY_REQUIRED: c_int = -0x6A80;
  pub const ERR_SSL_PEER_CLOSE_NOTIFY: c_int = -0x7880;
  pub const ERR_SSL_CONN_EOF: c_int = -0x7280;
  pub const ERR_NET_SEND_FAILED: c_int = -0x004E;
  pub const ERR_NET_RECV_FAILED: c_int = -0x004C;

  pub const SSL_IS_SERVER: c_int = 1;
  pub const SSL_IS_CLIENT: c_int = 0;
  pub const SSL_TRANSPORT_DATAGRAM: c_int = 1;
  pub const SSL_PRESET_DEFAULT: c_int = 0;
  pub const SSL_CID_ENABLED: c_int = 1;
  // Only the constants test references the disabled form; it exists so a
  // header drift on either value fails loudly.
  #[allow(dead_code)]
  pub const SSL_CID_DISABLED: c_int = 0;
  pub const SSL_UNEXPECTED_CID_IGNORE: c_int = 0;

  pub const TLS_PSK_WITH_AES_128_CCM_8: c_int = 0xC0A8;
  pub const TLS_PSK_WITH_AES_128_GCM_SHA256: c_int = 0x00A8;
  pub const TLS_PSK_WITH_AES_128_CBC_SHA256: c_int = 0x00AE;
}

/// A non-retriable mbedTLS return code, displayable with the library's own
/// error string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbedError(pub c_int);

impl fmt::Display for MbedError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut buf = [0i8; 128];
    // SAFETY: `buf` is a valid, writable, NUL-terminated-by-callee buffer
    // of the length passed; `mbedtls_strerror` never reads it and never
    // retains the pointer.
    unsafe { crate::ffi::mbedtls_strerror(self.0, buf.as_mut_ptr(), buf.len()) };
    let text = buf
      .iter()
      .take_while(|&&c| c != 0)
      .map(|&c| c as u8 as char)
      .collect::<String>();
    if text.is_empty() {
      write!(f, "mbedTLS error -{:#06x}", -self.0)
    } else {
      write!(f, "{text} (-{:#06x})", -self.0)
    }
  }
}

impl std::error::Error for MbedError {}

#[cfg(test)]
mod tests {
  use super::codes::*;

  #[test]
  fn constants_match_headers() {
    let expected: [(std::os::raw::c_int, std::os::raw::c_int); 18] = [
      (0, SSL_IS_SERVER),
      (1, SSL_IS_CLIENT),
      (2, SSL_TRANSPORT_DATAGRAM),
      (3, SSL_PRESET_DEFAULT),
      (4, SSL_CID_ENABLED),
      (5, SSL_CID_DISABLED),
      (6, SSL_UNEXPECTED_CID_IGNORE),
      (7, ERR_SSL_WANT_READ),
      (8, ERR_SSL_WANT_WRITE),
      (9, ERR_SSL_TIMEOUT),
      (10, ERR_SSL_HELLO_VERIFY_REQUIRED),
      (11, ERR_SSL_PEER_CLOSE_NOTIFY),
      (12, ERR_SSL_CONN_EOF),
      (13, ERR_NET_SEND_FAILED),
      (14, ERR_NET_RECV_FAILED),
      (15, TLS_PSK_WITH_AES_128_CCM_8),
      (16, TLS_PSK_WITH_AES_128_GCM_SHA256),
      (17, TLS_PSK_WITH_AES_128_CBC_SHA256),
    ];
    for (which, value) in expected {
      // SAFETY: `shim_const` is a pure lookup with no pointer arguments.
      let header_value = unsafe { crate::ffi::shim_const(which) };
      assert_eq!(header_value, value, "constant index {which} drifted");
    }
  }
}
