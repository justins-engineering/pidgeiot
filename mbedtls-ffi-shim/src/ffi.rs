//! Hand-declared externs against exported `libmbedtls` symbols, plus the
//! handful of `shim_*` functions from the compiled glue TU (csrc/glue.c).
//! Every mbedTLS context type is opaque on this side -- the glue TU is the
//! only place that knows their sizes -- and every signature below is taken
//! from the mbedTLS 3.6 headers the same build compiles the glue against,
//! so a header/declaration mismatch cannot survive an image build. The
//! Phase 0 gate additionally proved each of these is a real dynamic symbol
//! of Debian trixie's libmbedtls.so.21 (`docs/infra/coap-cid-design.md`).

use std::ffi::c_void;
use std::marker::PhantomData;
use std::os::raw::{c_char, c_int, c_uchar};

/// Opaque-type shape per the FFI omnibus pattern: zero-sized, unconstructable
/// from Rust, `!Send`/`!Sync` by way of the raw-pointer marker so nothing
/// can smuggle a context across threads without the safe wrappers' say-so.
macro_rules! opaque {
  ($name:ident) => {
    #[repr(C)]
    pub struct $name {
      _data: [u8; 0],
      _marker: PhantomData<*mut u8>,
    }
  };
}

opaque!(SslContext);
opaque!(SslConfig);
opaque!(CookieCtx);

pub type SendCb = unsafe extern "C" fn(*mut c_void, *const c_uchar, usize) -> c_int;
pub type RecvCb = unsafe extern "C" fn(*mut c_void, *mut c_uchar, usize) -> c_int;
pub type RecvTimeoutCb = unsafe extern "C" fn(*mut c_void, *mut c_uchar, usize, u32) -> c_int;
pub type TimerSetCb = unsafe extern "C" fn(*mut c_void, u32, u32);
pub type TimerGetCb = unsafe extern "C" fn(*mut c_void) -> c_int;
pub type RngCb = unsafe extern "C" fn(*mut c_void, *mut c_uchar, usize) -> c_int;
pub type PskCb = unsafe extern "C" fn(*mut c_void, *mut SslContext, *const c_uchar, usize) -> c_int;
pub type CookieWriteCb = unsafe extern "C" fn(
  *mut c_void,
  *mut *mut c_uchar,
  *mut c_uchar,
  *const c_uchar,
  usize,
) -> c_int;
pub type CookieCheckCb =
  unsafe extern "C" fn(*mut c_void, *const c_uchar, usize, *const c_uchar, usize) -> c_int;

unsafe extern "C" {
  // Glue TU (csrc/glue.c): allocators for config-sized contexts and
  // wrappers for header-inline setters.
  pub fn shim_ssl_new() -> *mut SslContext;
  pub fn shim_ssl_free(ssl: *mut SslContext);
  pub fn shim_ssl_config_new() -> *mut SslConfig;
  pub fn shim_ssl_config_free(conf: *mut SslConfig);
  pub fn shim_cookie_new() -> *mut CookieCtx;
  pub fn shim_cookie_free(ctx: *mut CookieCtx);
  pub fn shim_ssl_conf_tls12_only(conf: *mut SslConfig);
  pub fn shim_ssl_set_user_data(ssl: *mut SslContext, p: *mut c_void);
  pub fn shim_ssl_get_user_data(ssl: *mut SslContext) -> *mut c_void;
  // Referenced only by the constants unit test, which is its whole job.
  #[allow(dead_code)]
  pub fn shim_const(which: c_int) -> c_int;

  // libmbedtls exported symbols.
  pub fn mbedtls_ssl_config_defaults(
    conf: *mut SslConfig,
    endpoint: c_int,
    transport: c_int,
    preset: c_int,
  ) -> c_int;
  pub fn mbedtls_ssl_conf_rng(conf: *mut SslConfig, f_rng: RngCb, p_rng: *mut c_void);
  pub fn mbedtls_ssl_conf_ciphersuites(conf: *mut SslConfig, list: *const c_int);
  pub fn mbedtls_ssl_conf_cid(conf: *mut SslConfig, len: usize, ignore_other: c_int) -> c_int;
  pub fn mbedtls_ssl_conf_psk(
    conf: *mut SslConfig,
    psk: *const c_uchar,
    psk_len: usize,
    identity: *const c_uchar,
    identity_len: usize,
  ) -> c_int;
  pub fn mbedtls_ssl_conf_psk_cb(conf: *mut SslConfig, cb: PskCb, p: *mut c_void);
  pub fn mbedtls_ssl_conf_dtls_cookies(
    conf: *mut SslConfig,
    f_write: CookieWriteCb,
    f_check: CookieCheckCb,
    p: *mut c_void,
  );
  pub fn mbedtls_ssl_conf_handshake_timeout(conf: *mut SslConfig, min_ms: u32, max_ms: u32);

  pub fn mbedtls_ssl_setup(ssl: *mut SslContext, conf: *const SslConfig) -> c_int;
  pub fn mbedtls_ssl_session_reset(ssl: *mut SslContext) -> c_int;
  pub fn mbedtls_ssl_set_bio(
    ssl: *mut SslContext,
    p_bio: *mut c_void,
    f_send: SendCb,
    f_recv: Option<RecvCb>,
    f_recv_timeout: Option<RecvTimeoutCb>,
  );
  pub fn mbedtls_ssl_set_timer_cb(
    ssl: *mut SslContext,
    p_timer: *mut c_void,
    f_set: TimerSetCb,
    f_get: TimerGetCb,
  );
  pub fn mbedtls_ssl_set_mtu(ssl: *mut SslContext, mtu: u16);
  pub fn mbedtls_ssl_set_client_transport_id(
    ssl: *mut SslContext,
    info: *const c_uchar,
    ilen: usize,
  ) -> c_int;
  pub fn mbedtls_ssl_set_cid(
    ssl: *mut SslContext,
    enable: c_int,
    own_cid: *const c_uchar,
    own_cid_len: usize,
  ) -> c_int;
  pub fn mbedtls_ssl_get_peer_cid(
    ssl: *mut SslContext,
    enabled: *mut c_int,
    peer_cid: *mut c_uchar,
    peer_cid_len: *mut usize,
  ) -> c_int;
  pub fn mbedtls_ssl_set_hs_psk(ssl: *mut SslContext, psk: *const c_uchar, len: usize) -> c_int;
  pub fn mbedtls_ssl_handshake(ssl: *mut SslContext) -> c_int;
  pub fn mbedtls_ssl_read(ssl: *mut SslContext, buf: *mut c_uchar, len: usize) -> c_int;
  pub fn mbedtls_ssl_write(ssl: *mut SslContext, buf: *const c_uchar, len: usize) -> c_int;
  pub fn mbedtls_ssl_close_notify(ssl: *mut SslContext) -> c_int;
  pub fn mbedtls_ssl_get_ciphersuite(ssl: *const SslContext) -> *const c_char;

  pub fn mbedtls_ssl_cookie_setup(ctx: *mut CookieCtx, f_rng: RngCb, p_rng: *mut c_void) -> c_int;
  pub fn mbedtls_ssl_cookie_write(
    p: *mut c_void,
    cookie: *mut *mut c_uchar,
    end: *mut c_uchar,
    info: *const c_uchar,
    ilen: usize,
  ) -> c_int;
  pub fn mbedtls_ssl_cookie_check(
    p: *mut c_void,
    cookie: *const c_uchar,
    clen: usize,
    info: *const c_uchar,
    ilen: usize,
  ) -> c_int;

  pub fn mbedtls_strerror(errnum: c_int, buffer: *mut c_char, buflen: usize);
  pub fn mbedtls_version_get_number() -> u32;
  pub fn mbedtls_version_get_string(string: *mut c_char);
}
