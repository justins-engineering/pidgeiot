//! One DTLS association: an opaque `mbedtls_ssl_context` bound to a
//! caller-supplied transport. Each session is owned by exactly one thread
//! for its lifetime -- `Send` so it can move from the listener thread to
//! its connection thread at promotion, deliberately `!Sync` so no two
//! threads can ever drive one context concurrently (the structural answer
//! to mbedTLS 3.x's shared-context thread-safety story).

use std::ffi::{CStr, c_void};
use std::os::raw::{c_int, c_uchar};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{CID_LEN, Config};
use crate::error::{MbedError, codes};
use crate::ffi;

/// What a transport's `recv` observed. `TimerExpired` maps to
/// `MBEDTLS_ERR_SSL_TIMEOUT`, which is what makes mbedTLS drive its own
/// flight retransmission through the timer callbacks.
pub enum RecvOutcome {
  Data(usize),
  WantRead,
  TimerExpired,
  Closed,
  Failed,
}

pub enum SendOutcome {
  Sent(usize),
  Failed,
}

/// The transport a [`Session`] runs over. `recv` must deliver exactly one
/// whole datagram or nothing -- if the buffer is too small, drop the whole
/// datagram and keep waiting, never truncate (a datagram prefix is a
/// corrupt record, not a short read).
pub trait MbedIo: Send {
  fn send(&mut self, buf: &[u8]) -> SendOutcome;
  fn recv(&mut self, buf: &mut [u8], timer: &TimerState) -> RecvOutcome;
}

/// The mandatory DTLS timer pair's state, owned next to the transport so
/// a blocking `recv` can bound its park by the retransmission deadline.
pub struct TimerState {
  int_ms: u32,
  fin_ms: u32,
  start: Option<Instant>,
}

impl TimerState {
  fn new() -> TimerState {
    TimerState {
      int_ms: 0,
      fin_ms: 0,
      start: None,
    }
  }

  fn set(&mut self, int_ms: u32, fin_ms: u32) {
    self.int_ms = int_ms;
    self.fin_ms = fin_ms;
    // fin_ms == 0 cancels the timer, per the mbedtls_ssl_set_timer_cb
    // contract.
    self.start = (fin_ms != 0).then(Instant::now);
  }

  /// The three-way status `f_get_timer` must report: -1 cancelled, 0
  /// running, 1 intermediate delay passed, 2 final delay passed.
  fn status(&self) -> c_int {
    match self.start {
      None => -1,
      Some(started) => {
        let elapsed = started.elapsed().as_millis();
        if elapsed >= u128::from(self.fin_ms) {
          2
        } else if elapsed >= u128::from(self.int_ms) {
          1
        } else {
          0
        }
      }
    }
  }

  pub fn final_expired(&self) -> bool {
    self.status() == 2
  }

  /// Time until the final delay fires, if the timer is running.
  pub fn remaining(&self) -> Option<Duration> {
    let started = self.start?;
    Some(Duration::from_millis(u64::from(self.fin_ms)).saturating_sub(started.elapsed()))
  }
}

/// Per-session slot the config-level PSK callback stashes the
/// authenticated (identity, token) pair into, reached through mbedTLS's
/// per-connection user data.
pub struct SessionHeader {
  pub(crate) credentials: Option<(String, String)>,
}

struct SessionCtx<C> {
  header: SessionHeader,
  timer: TimerState,
  io: C,
}

pub enum HandshakeStatus {
  Done,
  WantRead,
  WantWrite,
  /// A HelloVerifyRequest just went out through `send`; reset the session
  /// and stay stateless.
  HelloVerifyRequired,
  Failed(MbedError),
}

pub enum ReadStatus {
  Data(usize),
  WantRead,
  WantWrite,
  PeerClosed,
  Failed(MbedError),
}

/// Whether the peer negotiated RFC 9146 CID on a completed handshake, and
/// the CID it asked us to send (empty for the fleet's zero-length offer).
pub struct CidStatus {
  pub negotiated: bool,
  pub peer_cid: Vec<u8>,
}

pub struct Session<C: MbedIo> {
  ssl: *mut ffi::SslContext,
  ctx: Box<SessionCtx<C>>,
  _config: Arc<Config>,
}

// SAFETY: a `Session` owns its `SSL` context exclusively (the raw pointer
// never escapes), the transport is `Send` by bound, and the config it
// keeps alive is `Sync`. No `Sync` impl on purpose: one thread at a time.
unsafe impl<C: MbedIo> Send for Session<C> {}

impl<C: MbedIo> Session<C> {
  pub fn new(config: &Arc<Config>, io: C) -> Result<Session<C>, MbedError> {
    // SAFETY: allocator wrapper; null checked below.
    let ssl = unsafe { ffi::shim_ssl_new() };
    if ssl.is_null() {
      return Err(MbedError(codes::ERR_NET_RECV_FAILED));
    }
    // SAFETY: fresh context, valid shared config outliving it via the Arc.
    let rc = unsafe { ffi::mbedtls_ssl_setup(ssl, config.conf) };
    if rc != 0 {
      // SAFETY: freeing the context just allocated; nothing references it.
      unsafe { ffi::shim_ssl_free(ssl) };
      return Err(MbedError(rc));
    }

    let mut ctx = Box::new(SessionCtx {
      header: SessionHeader { credentials: None },
      timer: TimerState::new(),
      io,
    });
    let ctx_ptr = &mut *ctx as *mut SessionCtx<C> as *mut c_void;
    // SAFETY: the box is heap-pinned for the session's lifetime and every
    // registered pointer derives from it; the monomorphized trampolines
    // downcast back to exactly `SessionCtx<C>`. mbedTLS invokes them only
    // from within calls this session makes on its owning thread, so no
    // two of these borrows are ever live at once.
    unsafe {
      ffi::mbedtls_ssl_set_bio(ssl, ctx_ptr, send_tramp::<C>, None, Some(recv_tramp::<C>));
      ffi::mbedtls_ssl_set_timer_cb(ssl, ctx_ptr, timer_set_tramp::<C>, timer_get_tramp::<C>);
      ffi::shim_ssl_set_user_data(ssl, &mut ctx.header as *mut SessionHeader as *mut c_void);
    }

    Ok(Session {
      ssl,
      ctx,
      _config: config.clone(),
    })
  }

  /// Resets for reuse on the pending-listen path. The caller must re-apply
  /// the per-attempt state afterwards: MTU, client transport id, own CID.
  pub fn reset(&mut self) -> Result<(), MbedError> {
    // SAFETY: exclusive borrow; no callback in flight.
    let rc = unsafe { ffi::mbedtls_ssl_session_reset(self.ssl) };
    if rc != 0 {
      return Err(MbedError(rc));
    }
    // The user data pointer is application state mbedTLS preserves across
    // resets, but re-stamping it costs nothing and keeps the invariant
    // local; the stale credential stash from a failed attempt must go
    // either way.
    self.ctx.header.credentials = None;
    // SAFETY: same pinned header as at construction.
    unsafe {
      ffi::shim_ssl_set_user_data(
        self.ssl,
        &mut self.ctx.header as *mut SessionHeader as *mut c_void,
      );
    }
    Ok(())
  }

  pub fn set_mtu(&mut self, mtu: u16) {
    // SAFETY: exclusive borrow.
    unsafe { ffi::mbedtls_ssl_set_mtu(self.ssl, mtu) };
  }

  /// The cookie's address binding: must name the claimed source before
  /// every pending-listen attempt (and again after every reset).
  pub fn set_client_transport_id(&mut self, info: &[u8]) -> Result<(), MbedError> {
    // SAFETY: exclusive borrow; mbedTLS copies the buffer.
    let rc =
      unsafe { ffi::mbedtls_ssl_set_client_transport_id(self.ssl, info.as_ptr(), info.len()) };
    if rc == 0 { Ok(()) } else { Err(MbedError(rc)) }
  }

  /// Server side: the CID this session will ask the peer to prefix its
  /// records with. Must precede the handshake and be re-applied after
  /// every reset.
  pub fn set_own_cid(&mut self, cid: &[u8; CID_LEN]) -> Result<(), MbedError> {
    // SAFETY: exclusive borrow; mbedTLS copies the CID.
    let rc = unsafe {
      ffi::mbedtls_ssl_set_cid(self.ssl, codes::SSL_CID_ENABLED, cid.as_ptr(), cid.len())
    };
    if rc == 0 { Ok(()) } else { Err(MbedError(rc)) }
  }

  /// Client side: offer CID with a zero-length CID of our own -- the
  /// fleet's shape (server->client records stay plain content type 23).
  pub fn offer_zero_length_cid(&mut self) -> Result<(), MbedError> {
    // SAFETY: exclusive borrow; a null CID with zero length is the
    // documented zero-length-offer form.
    let rc =
      unsafe { ffi::mbedtls_ssl_set_cid(self.ssl, codes::SSL_CID_ENABLED, std::ptr::null(), 0) };
    if rc == 0 { Ok(()) } else { Err(MbedError(rc)) }
  }

  pub fn handshake(&mut self) -> HandshakeStatus {
    // SAFETY: exclusive borrow; callbacks re-enter only through the
    // pinned ctx.
    let rc = unsafe { ffi::mbedtls_ssl_handshake(self.ssl) };
    match rc {
      0 => HandshakeStatus::Done,
      codes::ERR_SSL_WANT_READ => HandshakeStatus::WantRead,
      codes::ERR_SSL_WANT_WRITE => HandshakeStatus::WantWrite,
      codes::ERR_SSL_HELLO_VERIFY_REQUIRED => HandshakeStatus::HelloVerifyRequired,
      e => HandshakeStatus::Failed(MbedError(e)),
    }
  }

  pub fn read(&mut self, buf: &mut [u8]) -> ReadStatus {
    // SAFETY: exclusive borrow; `buf` is valid for the call.
    let rc = unsafe { ffi::mbedtls_ssl_read(self.ssl, buf.as_mut_ptr(), buf.len()) };
    match rc {
      n if n >= 0 => ReadStatus::Data(n as usize),
      codes::ERR_SSL_WANT_READ => ReadStatus::WantRead,
      codes::ERR_SSL_WANT_WRITE => ReadStatus::WantWrite,
      codes::ERR_SSL_PEER_CLOSE_NOTIFY | codes::ERR_SSL_CONN_EOF => ReadStatus::PeerClosed,
      e => ReadStatus::Failed(MbedError(e)),
    }
  }

  /// One datagram out. DTLS is all-or-nothing per record, so a short
  /// write is a caller bug surfaced as the library's own error.
  pub fn write(&mut self, buf: &[u8]) -> Result<usize, MbedError> {
    // SAFETY: exclusive borrow; `buf` is valid for the call.
    let rc = unsafe { ffi::mbedtls_ssl_write(self.ssl, buf.as_ptr(), buf.len()) };
    if rc >= 0 {
      Ok(rc as usize)
    } else {
      Err(MbedError(rc))
    }
  }

  /// Best-effort close_notify; failures are the peer's problem by then.
  pub fn close_notify(&mut self) {
    // SAFETY: exclusive borrow.
    let _ = unsafe { ffi::mbedtls_ssl_close_notify(self.ssl) };
  }

  /// CID negotiation outcome; meaningful only after the handshake
  /// completed.
  pub fn peer_cid(&mut self) -> Result<CidStatus, MbedError> {
    let mut enabled: c_int = 0;
    // The library writes up to its compile-time CID cap into this buffer
    // without taking its size; the glue TU compile-gates that cap at
    // <= 32 (and >= CID_LEN), so a rebuilt library can never outgrow it.
    let mut cid = [0u8; 32];
    let mut len: usize = 0;
    // SAFETY: exclusive borrow; out-pointers are valid, correctly sized.
    let rc =
      unsafe { ffi::mbedtls_ssl_get_peer_cid(self.ssl, &mut enabled, cid.as_mut_ptr(), &mut len) };
    if rc != 0 {
      return Err(MbedError(rc));
    }
    let len = len.min(cid.len());
    Ok(CidStatus {
      negotiated: enabled == codes::SSL_CID_ENABLED,
      peer_cid: cid[..len].to_vec(),
    })
  }

  pub fn ciphersuite(&self) -> Option<String> {
    // SAFETY: shared borrow is fine -- the library only reads; the
    // returned pointer is a static string inside libmbedtls.
    let p = unsafe { ffi::mbedtls_ssl_get_ciphersuite(self.ssl) };
    if p.is_null() {
      return None;
    }
    // SAFETY: mbedTLS guarantees a NUL-terminated static string.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
  }

  /// The (identity, token) pair the PSK callback authenticated for this
  /// session, surrendered once.
  pub fn take_credentials(&mut self) -> Option<(String, String)> {
    self.ctx.header.credentials.take()
  }

  pub fn io(&self) -> &C {
    &self.ctx.io
  }

  pub fn io_mut(&mut self) -> &mut C {
    &mut self.ctx.io
  }
}

impl<C: MbedIo> Drop for Session<C> {
  fn drop(&mut self) {
    // SAFETY: exclusive ownership; the ctx box (which the context's
    // registered pointers point into) is still alive here and drops after.
    unsafe { ffi::shim_ssl_free(self.ssl) };
  }
}

unsafe extern "C" fn send_tramp<C: MbedIo>(
  p: *mut c_void,
  buf: *const c_uchar,
  len: usize,
) -> c_int {
  let outcome = catch_unwind(AssertUnwindSafe(|| {
    // SAFETY: `p` is this session's pinned `SessionCtx<C>`; mbedTLS calls
    // its callbacks strictly sequentially from within the session's own
    // calls, so this exclusive borrow never overlaps another.
    let ctx = unsafe { &mut *(p as *mut SessionCtx<C>) };
    // SAFETY: `buf`/`len` describe the record mbedTLS is emitting, valid
    // for the duration of the call.
    let bytes = unsafe { slice::from_raw_parts(buf, len) };
    ctx.io.send(bytes)
  }));
  match outcome {
    Ok(SendOutcome::Sent(n)) => n.min(c_int::MAX as usize) as c_int,
    Ok(SendOutcome::Failed) | Err(_) => codes::ERR_NET_SEND_FAILED,
  }
}

unsafe extern "C" fn recv_tramp<C: MbedIo>(
  p: *mut c_void,
  buf: *mut c_uchar,
  len: usize,
  _timeout_ms: u32,
) -> c_int {
  let outcome = catch_unwind(AssertUnwindSafe(|| {
    // SAFETY: as in `send_tramp`.
    let ctx = unsafe { &mut *(p as *mut SessionCtx<C>) };
    // SAFETY: `buf`/`len` is the library's receive buffer, valid and
    // writable for the duration of the call.
    let bytes = unsafe { slice::from_raw_parts_mut(buf, len) };
    // The timeout hint mbedTLS passes is ignored on purpose: the
    // transport owns its park budget (tick, retransmission timer,
    // wall-clock deadline) and reports TimerExpired from the same timer
    // state mbedTLS reads, so the two sides cannot disagree.
    ctx.io.recv(bytes, &ctx.timer)
  }));
  match outcome {
    Ok(RecvOutcome::Data(n)) => n.min(c_int::MAX as usize) as c_int,
    Ok(RecvOutcome::WantRead) => codes::ERR_SSL_WANT_READ,
    Ok(RecvOutcome::TimerExpired) => codes::ERR_SSL_TIMEOUT,
    Ok(RecvOutcome::Closed) => 0,
    Ok(RecvOutcome::Failed) | Err(_) => codes::ERR_NET_RECV_FAILED,
  }
}

unsafe extern "C" fn timer_set_tramp<C: MbedIo>(p: *mut c_void, int_ms: u32, fin_ms: u32) {
  // SAFETY: as in `send_tramp`. A panic is impossible here (plain field
  // stores), so no catch_unwind.
  let ctx = unsafe { &mut *(p as *mut SessionCtx<C>) };
  ctx.timer.set(int_ms, fin_ms);
}

unsafe extern "C" fn timer_get_tramp<C: MbedIo>(p: *mut c_void) -> c_int {
  // SAFETY: as in `send_tramp`.
  let ctx = unsafe { &mut *(p as *mut SessionCtx<C>) };
  ctx.timer.status()
}
