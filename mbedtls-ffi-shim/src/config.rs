//! Shared `mbedtls_ssl_config` wrapper: built once, immutable thereafter,
//! shared by every session (mbedTLS documents an `mbedtls_ssl_config` as
//! shareable across contexts precisely when it is no longer mutated).
//! Server configs carry the PSK resolver callback, the HelloVerifyRequest
//! cookie machinery, and the RFC 9146 CID offer; client configs exist for
//! the test suites on both sides of a loopback handshake.

use std::ffi::c_void;
use std::os::raw::{c_int, c_uchar};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{MbedError, codes};
use crate::ffi;
use crate::session::SessionHeader;

/// The DTLS-PSK suites loft pins, most-preferred first, zero-terminated
/// for `mbedtls_ssl_conf_ciphersuites`. Mirrors `PSK_CIPHER_LIST` on the
/// OpenSSL side.
pub const PSK_SUITES: [c_int; 4] = [
  codes::TLS_PSK_WITH_AES_128_CCM_8,
  codes::TLS_PSK_WITH_AES_128_GCM_SHA256,
  codes::TLS_PSK_WITH_AES_128_CBC_SHA256,
  0,
];

/// The one CID length this stack speaks. The demux parses it at a fixed
/// record offset, so it is deliberately a compile-time constant, not
/// configuration.
pub const CID_LEN: usize = 8;

/// A successful PSK resolution: the key that finishes the handshake plus
/// the identity/token pair the session carries upstream afterwards.
pub struct ResolvedPsk {
  pub psk: Vec<u8>,
  pub identity: String,
  pub token: String,
}

pub type PskCallback = Box<dyn Fn(&[u8]) -> Option<ResolvedPsk> + Send + Sync>;

struct PskShared {
  resolve: PskCallback,
}

/// Cookie context plus the verified-flag the pending-listen flow needs:
/// mbedTLS reports both "garbage discarded" and "cookie-verified
/// ClientHello consumed, reply flight written" as WANT_READ, so the check
/// callback records which one happened. Atomic not for cross-thread
/// traffic (the pending flow is listener-thread-confined) but because the
/// config -- and therefore this pointer -- is shared with session threads,
/// and mbedTLS's port-reuse path can run a cookie check from one of them.
struct CookieState {
  ctx: *mut ffi::CookieCtx,
  verified: AtomicBool,
}

impl Drop for CookieState {
  fn drop(&mut self) {
    // SAFETY: allocated by `shim_cookie_new` in `Config::server`, owned
    // exclusively by this struct, freed exactly once here.
    unsafe { ffi::shim_cookie_free(self.ctx) };
  }
}

pub(crate) enum Role {
  Server,
  Client,
}

pub struct Config {
  pub(crate) conf: *mut ffi::SslConfig,
  pub(crate) role: Role,
  cookie: Option<Box<CookieState>>,
  psk: Option<Box<PskShared>>,
  // `mbedtls_ssl_conf_ciphersuites` stores the caller's pointer; the list
  // must live exactly as long as the config.
  _suites: Box<[c_int]>,
}

// SAFETY: the config is immutable after its constructor returns -- every
// setter is private to this module and called only during construction --
// which is mbedTLS's own documented condition for sharing one config
// across contexts and threads. The interior-mutable pieces reachable
// through it are the cookie verified flag (atomic) and mbedTLS's own
// cookie key rotation (guarded by the library's MBEDTLS_THREADING_PTHREAD
// mutexes on Debian's build).
unsafe impl Send for Config {}
unsafe impl Sync for Config {}

impl Drop for Config {
  fn drop(&mut self) {
    // SAFETY: `conf` was allocated by `shim_ssl_config_new`, is owned
    // exclusively, and no `Session` can outlive it (each holds an
    // `Arc<Config>`).
    unsafe { ffi::shim_ssl_config_free(self.conf) };
  }
}

impl Config {
  /// A DTLS 1.2 PSK server config: pinned suites, 8-byte CID offered,
  /// HelloVerifyRequest cookies on, RNG over `getrandom(2)`, anti-replay
  /// and renegotiation left at their defaults (on and off respectively).
  pub fn server(resolve: PskCallback) -> Result<Config, MbedError> {
    let mut config = Config::base(Role::Server)?;

    // SAFETY: `conf` is a valid config; `CID_LEN` is within the header
    // caps (compile-gated in the glue TU).
    let rc =
      unsafe { ffi::mbedtls_ssl_conf_cid(config.conf, CID_LEN, codes::SSL_UNEXPECTED_CID_IGNORE) };
    if rc != 0 {
      return Err(MbedError(rc));
    }

    let cookie = Box::new(CookieState {
      // SAFETY: allocator wrapper; null checked below.
      ctx: unsafe { ffi::shim_cookie_new() },
      verified: AtomicBool::new(false),
    });
    if cookie.ctx.is_null() {
      return Err(MbedError(codes::ERR_NET_RECV_FAILED));
    }
    // SAFETY: fresh cookie ctx; the RNG callback is stateless.
    let rc =
      unsafe { ffi::mbedtls_ssl_cookie_setup(cookie.ctx, rng_getrandom, std::ptr::null_mut()) };
    if rc != 0 {
      return Err(MbedError(rc));
    }
    // SAFETY: the `CookieState` box is heap-pinned and owned by the config
    // being built, so the pointer registered here outlives every session
    // created from it.
    unsafe {
      ffi::mbedtls_ssl_conf_dtls_cookies(
        config.conf,
        cookie_write_tramp,
        cookie_check_tramp,
        &*cookie as *const CookieState as *mut c_void,
      );
    }
    config.cookie = Some(cookie);

    let psk = Box::new(PskShared { resolve });
    // SAFETY: same heap-pinned-ownership argument as the cookie state.
    unsafe {
      ffi::mbedtls_ssl_conf_psk_cb(
        config.conf,
        psk_tramp,
        &*psk as *const PskShared as *mut c_void,
      );
    }
    config.psk = Some(psk);

    Ok(config)
  }

  /// A DTLS 1.2 PSK client config with a fixed identity/key, optionally
  /// offering a zero-length CID of its own -- the fleet's exact shape.
  /// Test-side counterpart to [`Config::server`].
  pub fn client(identity: &[u8], psk: &[u8], offer_cid: bool) -> Result<Config, MbedError> {
    let config = Config::base(Role::Client)?;
    if offer_cid {
      // SAFETY: valid config; zero own-CID length is the RFC 9146 "I send
      // you nothing, route me by yours" form.
      let rc =
        unsafe { ffi::mbedtls_ssl_conf_cid(config.conf, 0, codes::SSL_UNEXPECTED_CID_IGNORE) };
      if rc != 0 {
        return Err(MbedError(rc));
      }
    }
    // SAFETY: `conf_psk` copies both buffers into the config.
    let rc = unsafe {
      ffi::mbedtls_ssl_conf_psk(
        config.conf,
        psk.as_ptr(),
        psk.len(),
        identity.as_ptr(),
        identity.len(),
      )
    };
    if rc != 0 {
      return Err(MbedError(rc));
    }
    Ok(config)
  }

  fn base(role: Role) -> Result<Config, MbedError> {
    // SAFETY: allocator wrapper; null checked below.
    let conf = unsafe { ffi::shim_ssl_config_new() };
    if conf.is_null() {
      return Err(MbedError(codes::ERR_NET_RECV_FAILED));
    }
    let suites: Box<[c_int]> = Box::new(PSK_SUITES);
    let config = Config {
      conf,
      role,
      cookie: None,
      psk: None,
      _suites: suites,
    };

    let endpoint = match config.role {
      Role::Server => codes::SSL_IS_SERVER,
      Role::Client => codes::SSL_IS_CLIENT,
    };
    // SAFETY: valid fresh config; constants proven against the headers by
    // the `constants_match_headers` test.
    let rc = unsafe {
      ffi::mbedtls_ssl_config_defaults(
        config.conf,
        endpoint,
        codes::SSL_TRANSPORT_DATAGRAM,
        codes::SSL_PRESET_DEFAULT,
      )
    };
    if rc != 0 {
      return Err(MbedError(rc));
    }
    // SAFETY: the RNG callback is stateless (getrandom(2)); the suite list
    // is owned by the config and outlives it.
    unsafe {
      ffi::mbedtls_ssl_conf_rng(config.conf, rng_getrandom, std::ptr::null_mut());
      ffi::shim_ssl_conf_tls12_only(config.conf);
      ffi::mbedtls_ssl_conf_ciphersuites(config.conf, config._suites.as_ptr());
    }
    Ok(config)
  }

  /// Shrinks the DTLS retransmission backoff window. Tests use this to
  /// exercise flight retransmission in milliseconds instead of the
  /// protocol-default seconds; production keeps the defaults.
  pub fn set_handshake_timeout(&mut self, min_ms: u32, max_ms: u32) {
    // SAFETY: `&mut self` proves construction hasn't finished sharing yet.
    unsafe { ffi::mbedtls_ssl_conf_handshake_timeout(self.conf, min_ms, max_ms) };
  }

  /// Clears the cookie-verified flag ahead of one pending-listen attempt.
  pub fn clear_cookie_verified(&self) {
    if let Some(c) = &self.cookie {
      c.verified.store(false, Ordering::SeqCst);
    }
  }

  /// True exactly when a cookie-verified ClientHello was consumed since
  /// the last clear -- the disambiguator between mbedTLS's two WANT_READ
  /// meanings on the pending path.
  pub fn take_cookie_verified(&self) -> bool {
    self
      .cookie
      .as_ref()
      .is_some_and(|c| c.verified.swap(false, Ordering::SeqCst))
  }
}

/// `f_rng` over `getrandom(2)`: no mbedTLS entropy/ctr_drbg contexts, so
/// there is no shared crypto-context state to reason about.
pub(crate) unsafe extern "C" fn rng_getrandom(
  _p: *mut c_void,
  out: *mut c_uchar,
  len: usize,
) -> c_int {
  let mut off = 0usize;
  while off < len {
    // SAFETY: `out` is a valid buffer of `len` bytes for the duration of
    // the call per the f_rng contract; the offset stays in bounds.
    let n = unsafe { libc::getrandom((out as *mut c_void).add(off), len - off, 0) };
    if n < 0 {
      if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
        continue;
      }
      return -1;
    }
    off += n as usize;
  }
  0
}

unsafe extern "C" fn cookie_write_tramp(
  p: *mut c_void,
  cookie: *mut *mut c_uchar,
  end: *mut c_uchar,
  info: *const c_uchar,
  ilen: usize,
) -> c_int {
  // SAFETY: `p` is the `CookieState` registered at config build, alive as
  // long as the config; all other pointers pass through unchanged to the
  // library function they were minted for.
  let state = unsafe { &*(p as *const CookieState) };
  unsafe { ffi::mbedtls_ssl_cookie_write(state.ctx as *mut c_void, cookie, end, info, ilen) }
}

unsafe extern "C" fn cookie_check_tramp(
  p: *mut c_void,
  cookie: *const c_uchar,
  clen: usize,
  info: *const c_uchar,
  ilen: usize,
) -> c_int {
  // SAFETY: as in `cookie_write_tramp`.
  let state = unsafe { &*(p as *const CookieState) };
  let rc =
    unsafe { ffi::mbedtls_ssl_cookie_check(state.ctx as *mut c_void, cookie, clen, info, ilen) };
  if rc == 0 {
    state.verified.store(true, Ordering::SeqCst);
  }
  rc
}

/// Server PSK callback: resolve, install the key for this handshake, and
/// stash the authenticated (identity, token) pair in the session's header
/// via the per-connection user data.
///
/// Every reject path -- resolver miss, resolver panic, `set_hs_psk`
/// failure -- installs a random key instead of failing the callback: a
/// plain nonzero return makes mbedTLS send an `unknown_psk_identity`
/// alert at ClientKeyExchange, while a wrong key only surfaces later as a
/// Finished-verification failure -- a probing oracle that tells an
/// attacker which identities exist. Under a random key an unknown
/// identity fails exactly where and how a wrong key does, and the random
/// key can never complete a handshake (the client's Finished cannot
/// verify against it).
unsafe extern "C" fn psk_tramp(
  p: *mut c_void,
  ssl: *mut ffi::SslContext,
  id: *const c_uchar,
  idlen: usize,
) -> c_int {
  // A panic must not unwind into C; treat it as a reject.
  let result = catch_unwind(AssertUnwindSafe(|| {
    // SAFETY: `p` is the `PskShared` registered at config build; the
    // identity buffer is valid for the callback's duration per contract.
    let shared = unsafe { &*(p as *const PskShared) };
    let identity: &[u8] = if id.is_null() {
      &[]
    } else {
      unsafe { slice::from_raw_parts(id, idlen) }
    };
    (shared.resolve)(identity)
  }));
  let Ok(Some(resolved)) = result else {
    return unsafe { reject_with_decoy_psk(ssl) };
  };
  // SAFETY: `ssl` is the in-handshake context mbedTLS handed us;
  // `set_hs_psk` copies the key.
  if unsafe { ffi::mbedtls_ssl_set_hs_psk(ssl, resolved.psk.as_ptr(), resolved.psk.len()) } != 0 {
    return unsafe { reject_with_decoy_psk(ssl) };
  }
  // SAFETY: the user data pointer is the session's heap-pinned
  // `SessionHeader`, set at session construction; no other reference to
  // it is live while mbedTLS is inside this callback.
  let header = unsafe { ffi::shim_ssl_get_user_data(ssl) } as *mut SessionHeader;
  if !header.is_null() {
    unsafe { (*header).credentials = Some((resolved.identity, resolved.token)) };
  }
  0
}

/// Always returns 0: a nonzero return from the callback is exactly the
/// `unknown_psk_identity` alert this function exists to avoid, so even
/// its own failure arms must not take that path. A failed `getrandom`
/// degrades to the all-zeros key and a failed `set_hs_psk` leaves no key
/// installed at all -- both still fail the handshake as a generic
/// failure, and neither can leak anything or serve anyone: a session
/// whose credentials were never stashed is torn down right after the
/// handshake even in the astronomically-unlikely case that a decoy key
/// completes one.
///
/// SAFETY: caller passes the live in-handshake context.
unsafe fn reject_with_decoy_psk(ssl: *mut ffi::SslContext) -> c_int {
  let mut key = [0u8; 32];
  // SAFETY: stack buffer of the stated length.
  let _ = unsafe { rng_getrandom(std::ptr::null_mut(), key.as_mut_ptr(), key.len()) };
  // SAFETY: as for the accept path; the key is copied.
  let _ = unsafe { ffi::mbedtls_ssl_set_hs_psk(ssl, key.as_ptr(), key.len()) };
  0
}
