//! Hand-written FFI shim for three OpenSSL DTLS primitives that the
//! `openssl` crate (0.10.x) does not expose:
//! `DTLSv1_get_timeout`, `DTLSv1_handle_timeout`, and `DTLSv1_listen`.
//!
//! # Why this exists
//!
//! `openssl-sys` binds none of the three (`ssl.rs`, `dtls1.rs`,
//! `handwritten/ssl.rs`). Without them, driving OpenSSL's DTLS
//! retransmission state machine from a single poll-driven event loop isn't
//! possible through the safe API alone -- you'd otherwise need one blocking
//! OS thread per connection.
//!
//! `DTLSv1_get_timeout`/`DTLSv1_handle_timeout` are plain macros around the
//! already-bound `SSL_ctrl`, so no new `extern` declarations are needed for
//! those two -- see `dtls_get_timeout`/`dtls_handle_timeout` below.
//! `DTLSv1_listen` is a genuine exported symbol that isn't bound; this module
//! declares it (and the `BIO_ADDR` allocator functions it needs) by hand,
//! reusing `openssl_sys::BIO_ADDR` and `openssl_sys::SSL`, which are already
//! public types in `openssl-sys`.
//!
//! Every signature and return-code contract documented on the functions
//! below is taken from openssl-3.6.3:
//!   - `include/openssl/ssl.h.in` (macro definitions, control codes, the
//!     `DTLSv1_listen` prototype)
//!   - `include/openssl/bio.h.in` (`BIO_ADDR_new`/`_free`/`_family`/
//!     `_rawaddress`/`_rawport` prototypes)
//!   - `doc/man3/DTLSv1_listen.pod`, `doc/man3/DTLSv1_get_timeout.pod`,
//!     `doc/man3/DTLSv1_handle_timeout.pod` (return-code semantics)
//!
//! # The pre-authentication surface
//!
//! This module's entire reason to exist is to touch bytes that arrive from
//! **unauthenticated UDP senders before any DTLS handshake, and therefore
//! before any cryptographic identity, has been established.** Every code path
//! here sits on that surface; the notes below cover what these wrappers
//! guarantee on it and what they must leave to the caller.
//!
//! ## No panics on attacker input
//!
//! `dtlsv1_listen` never panics or aborts regardless of what bytes are
//! sitting on the `Ssl`'s read side -- garbage, truncated records, a
//! ClientHello with a bogus or replayed cookie, or an empty datagram. It only
//! ever surfaces `DtlsShimError` or a `ListenOutcome`; every OpenSSL return
//! code is matched explicitly with no `unreachable!()`/`.unwrap()` on values
//! that originate from the C side. The `listen_garbage` integration tests
//! exercise exactly this.
//!
//! ## One exclusive user per `SSL *`
//!
//! Every wrapper here takes `&mut SslRef`, and that exclusivity is
//! load-bearing: `SslRef` is `Sync`, so with shared references these
//! functions would be callable concurrently from safe code (e.g. through an
//! `Arc<SslStream>` shared across threads), driving two OpenSSL handshake
//! operations on one `SSL *` at once -- a C-side data race, i.e. undefined
//! behavior, that the borrow checker cannot see through the FFI boundary.
//! The `&mut` requirement is the only thing in these signatures that rules
//! that out. rust-openssl's stream accessors only hand back `&SslRef`; use
//! [`ssl_mut`] to derive the exclusive reference from an exclusive borrow of
//! the owning `SslStream`.
//!
//! ## Stateless until the cookie verifies
//!
//! `DTLSv1_listen` operates statelessly on purpose -- that is the whole point
//! of RFC 6347's cookie exchange: reject bad handshakes before allocating
//! per-connection state. Nothing in this module allocates connection state
//! on the caller's behalf before a valid cookie is seen. The same rule
//! extends to the caller -- don't create the full application-level
//! `Pigeon`/connection context until `dtlsv1_listen` returns
//! `ListenOutcome::Accepted`.
//!
//! ## Peer-address discovery does not work through the `Read + Write` bridge
//!
//! `SslStream`/`Ssl::accept` in the safe `openssl` crate drive I/O through a
//! generic `Read + Write` adapter (see rust-openssl's private `bio.rs`), not
//! a real `BIO_s_datagram`. That adapter has no notion of "peer address" for
//! `BIO_ctrl(..., BIO_CTRL_DGRAM_GET_PEER, ...)` to read back, so
//! `DTLSv1_listen` will reliably report `AF_UNSPEC` (this module's
//! `ListenOutcome::Accepted { peer: None }`) for any `Ssl` wired up the
//! normal rust-openssl way. **This is expected, not a bug** -- the peer
//! assertion in the `listen_flow` integration test pins it down.
//! Practically: whoever builds the demux layer around this shim must already
//! know which socket address a datagram came from via their own
//! `UdpSocket::recv_from` (or equivalent) *before* routing bytes into a
//! specific `Ssl` object -- it cannot lean on this shim to learn the peer.
//!
//! ## Connect the socket after a successful listen
//!
//! Straight from `DTLSv1_listen(3)`: "It is essential that the calling code
//! connects the underlying socket to the peer after making use of
//! `DTLSv1_listen()`... failing to \[do so\] means that any host on the
//! network can cause outgoing DTLS traffic to be redirected to it." This
//! module has no way to enforce that from inside the shim -- it is a hard
//! requirement on the caller's transport-wiring code, called out here
//! because it is easy to miss and does not fail loudly if skipped.
//!
//! ## The version gate is a runtime check, not a build-time `cfg`
//!
//! `DTLSv1_listen`'s `BIO_ADDR *` parameter (and its return-code contract)
//! is an OpenSSL >= 1.1.0 ABI; earlier releases used a raw
//! `struct sockaddr *` there. `openssl-sys`'s own build-time version
//! detection isn't visible to downstream crates, and what actually matters
//! is the library this process ends up dynamically linked against at
//! runtime -- so `dtls_listen_supported()` checks `openssl::version::number()`
//! every time `dtlsv1_listen` is called. The cost is one integer compare;
//! the alternative is silent ABI mismatch UB on an ancient OpenSSL.
//!
//! # What this module deliberately does NOT do
//!
//! It does not decide how the caller multiplexes UDP datagrams to `Ssl`
//! objects, does not own a socket, and does not implement the
//! cookie-generate/verify callbacks (`SSL_CTX_set_cookie_generate_cb`/
//! `_verify_cb`) that `DTLSv1_listen` requires the caller to have configured
//! first -- those are `openssl::ssl::SslContextBuilder::set_cookie_generate_cb`/
//! `set_cookie_verify_cb`, already safely exposed by rust-openssl, and are the
//! architecture owner's call, not this shim's.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::raw::{c_int, c_void};
use std::ptr::NonNull;
use std::time::Duration;

use foreign_types::ForeignTypeRef;
use openssl::error::ErrorStack;
use openssl::ssl::{SslRef, SslStream};

/// `DTLS_CTRL_GET_TIMEOUT`, from `include/openssl/ssl.h.in` (openssl-3.6.3,
/// line 1295): `#define DTLS_CTRL_GET_TIMEOUT 73`. Unchanged since these
/// control codes were introduced; used only as the `cmd` argument to the
/// already-bound `openssl_sys::SSL_ctrl`.
const DTLS_CTRL_GET_TIMEOUT: c_int = 73;

/// `DTLS_CTRL_HANDLE_TIMEOUT`, from `include/openssl/ssl.h.in` (openssl-3.6.3,
/// line 1296): `#define DTLS_CTRL_HANDLE_TIMEOUT 74`.
const DTLS_CTRL_HANDLE_TIMEOUT: c_int = 74;

/// Packed `OPENSSL_VERSION_NUMBER` for OpenSSL 1.1.0 (`0x1010000fL`), the
/// first release with the `BIO_ADDR *`-based `DTLSv1_listen` ABI this module
/// assumes. See the module notes on the runtime version gate.
const MIN_SUPPORTED_OPENSSL_VERSION: i64 = 0x1010000f;

/// Hand-written externs for the one genuine symbol (`DTLSv1_listen`) and its
/// supporting `BIO_ADDR` allocator functions that `openssl-sys` does not bind.
/// Both `SSL` and `BIO_ADDR` are already public opaque types in `openssl-sys`
/// (`openssl_sys::handwritten::types`), so no new FFI types are introduced
/// here -- only new function declarations against types that already exist in
/// the dependency graph and are already linked (these symbols live in the
/// same `libssl`/`libcrypto` that `openssl-sys` itself links against).
mod raw {
  use openssl_sys::{BIO_ADDR, SSL};
  use std::os::raw::{c_int, c_void};

  extern "C" {
    /// `int DTLSv1_listen(SSL *s, BIO_ADDR *client);`
    /// `include/openssl/ssl.h.in`, openssl-3.6.3, line 2586, guarded by
    /// `#ifndef OPENSSL_NO_SOCK` (present in standard distro builds).
    pub fn DTLSv1_listen(ssl: *mut SSL, client: *mut BIO_ADDR) -> c_int;

    /// `BIO_ADDR *BIO_ADDR_new(void);` -- `include/openssl/bio.h.in`, line 819.
    pub fn BIO_ADDR_new() -> *mut BIO_ADDR;
    /// `void BIO_ADDR_free(BIO_ADDR *);` -- `include/openssl/bio.h.in`, line 824.
    pub fn BIO_ADDR_free(ap: *mut BIO_ADDR);
    /// `int BIO_ADDR_family(const BIO_ADDR *ap);` -- line 826.
    pub fn BIO_ADDR_family(ap: *const BIO_ADDR) -> c_int;
    /// `int BIO_ADDR_rawaddress(const BIO_ADDR *ap, void *p, size_t *l);` -- line 827.
    /// Writes the raw address bytes into `p` with a length dictated solely
    /// by the `BIO_ADDR`'s family (4 for `AF_INET`, 16 for `AF_INET6`);
    /// `*l` is a pure out-parameter reporting the bytes written, never an
    /// input capacity. Callers must pre-size `p` from `BIO_ADDR_family`.
    pub fn BIO_ADDR_rawaddress(ap: *const BIO_ADDR, p: *mut c_void, l: *mut usize) -> c_int;
    /// `unsigned short BIO_ADDR_rawport(const BIO_ADDR *ap);` -- line 828.
    /// Returned in network byte order per OpenSSL convention for this family
    /// of functions; converted with `u16::from_be` at the call site.
    pub fn BIO_ADDR_rawport(ap: *const BIO_ADDR) -> u16;
  }
}

/// Errors this module can produce. Deliberately distinct from bare
/// `ErrorStack` (which represents OpenSSL's own `ERR_get_error()` queue) so
/// that "this build/runtime combination isn't supported" is never confused
/// with a genuine OpenSSL-reported failure.
#[derive(Debug)]
pub enum DtlsShimError {
  /// The linked OpenSSL is older than [`MIN_SUPPORTED_OPENSSL_VERSION`]; see
  /// the module notes on the runtime version gate. Contains the packed
  /// version numbers actually linked and the minimum this shim supports,
  /// for logging.
  UnsupportedOpenSslVersion { linked: i64, minimum: i64 },
  /// A genuine OpenSSL-level failure (allocation failure, malformed input
  /// rejected internally, etc.), passed through from `ErrorStack::get()`.
  Openssl(ErrorStack),
}

impl fmt::Display for DtlsShimError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      DtlsShimError::UnsupportedOpenSslVersion { linked, minimum } => write!(
        f,
        "linked OpenSSL version {linked:#x} is older than the minimum \
                 {minimum:#x} (1.1.0) this DTLS FFI shim supports -- DTLSv1_listen's \
                 BIO_ADDR-based ABI is not guaranteed before that release"
      ),
      DtlsShimError::Openssl(e) => write!(f, "OpenSSL error: {e}"),
    }
  }
}

impl std::error::Error for DtlsShimError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      DtlsShimError::Openssl(e) => Some(e),
      DtlsShimError::UnsupportedOpenSslVersion { .. } => None,
    }
  }
}

impl From<ErrorStack> for DtlsShimError {
  fn from(e: ErrorStack) -> Self {
    DtlsShimError::Openssl(e)
  }
}

/// True if the OpenSSL actually linked at runtime is new enough for this
/// module's assumed `DTLSv1_listen` ABI (see the module notes on the runtime
/// version gate). Cheap (one FFI call to read a version constant); safe to
/// call on every `dtlsv1_listen` invocation, which it is.
pub fn dtls_listen_supported() -> bool {
  openssl::version::number() as i64 >= MIN_SUPPORTED_OPENSSL_VERSION
}

/// Derives the exclusive `&mut SslRef` the wrappers in this module require
/// from an exclusive borrow of the `SslStream` that owns the `SSL *`.
/// rust-openssl's own accessors (`SslStream::ssl()`,
/// `SslStreamBuilder::ssl()`) only hand back `&SslRef`, which -- `SslRef`
/// being `Sync` -- cannot prove the caller isn't driving the same `SSL *`
/// from another thread at the same time (see the module notes on
/// exclusivity). This is the sound bridge: an `SslStream` exclusively owns
/// its `Ssl`, so a mutable borrow of the stream guarantees no other Rust
/// reference to the same `SSL *` can be live while the returned reference
/// is.
pub fn ssl_mut<S>(stream: &mut SslStream<S>) -> &mut SslRef {
  // SAFETY: the pointer comes from a live `SslStream`, so it is valid,
  // non-null, and stays valid for as long as the stream does. The elided
  // lifetimes tie the returned `&mut SslRef` to the `&mut SslStream`
  // borrow, so the borrow checker enforces exclusivity for exactly the
  // region the reference exists.
  unsafe { SslRef::from_ptr_mut(stream.ssl().as_ptr()) }
}

/// Safe wrapper for `DTLSv1_get_timeout`. Returns `Some(duration)` if the
/// `ssl` object currently has a pending retransmission timer (per
/// `DTLSv1_get_timeout(3)`: "If the SSL object needs to be ticked
/// immediately, `*tv` is zeroed and the function succeeds" -- that zero
/// duration is passed through as-is, i.e. `Some(Duration::ZERO)` means "call
/// `dtls_handle_timeout` right now, don't wait"). Returns `None` if there is
/// no active timer, which per the man page is also what a non-DTLS/QUIC `Ssl`
/// or a call failure looks like -- this shim does not attempt to distinguish
/// those from "no timer pending" since `openssl-sys` gives no separate signal
/// for them and the caller's correct action (don't schedule a timeout) is
/// identical either way.
pub fn dtls_get_timeout(ssl: &mut SslRef) -> Option<Duration> {
  let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
  // SAFETY: `ssl.as_ptr()` is a valid, non-null `*mut SSL` for the lifetime
  // of this call (reborrowed from `&mut SslRef`, which cannot outlive its
  // owning `Ssl`/`SslStream`, and whose exclusivity guarantees no other
  // call is driving this `SSL *` concurrently -- see the module notes on
  // exclusivity). `&mut tv` is a valid, correctly-sized, correctly-
  // aligned out-parameter matching the platform's native `struct timeval`
  // layout (via `libc::timeval`, not a hand-rolled repr(C) struct, so this
  // is correct across the 32/64-bit `time_t`/`suseconds_t` variance that a
  // hand-rolled struct could get wrong). `SSL_ctrl` with `DTLS_CTRL_GET_TIMEOUT`
  // only ever reads from `ssl` and writes into `*tv`; it does not retain
  // either pointer past the call.
  let ret = unsafe {
    openssl_sys::SSL_ctrl(
      ssl.as_ptr(),
      DTLS_CTRL_GET_TIMEOUT,
      0,
      &mut tv as *mut libc::timeval as *mut c_void,
    )
  };
  if ret != 1 {
    return None;
  }
  // tv_sec/tv_usec are always non-negative for a real timeout value; guard
  // the cast anyway rather than trust that invariant blindly.
  let secs = tv.tv_sec.max(0) as u64;
  let micros = tv.tv_usec.clamp(0, 999_999) as u32;
  Some(Duration::new(secs, micros * 1_000))
}

/// Outcome of [`dtls_handle_timeout`], mirroring `DTLSv1_handle_timeout(3)`'s
/// three return codes (`1` / `0` / `-1`) without collapsing the "nothing to
/// do" and "handled successfully" cases into a bare bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleTimeoutOutcome {
  /// Return code `0`: no pending timeout (or `ssl` isn't a DTLS/QUIC
  /// object). Nothing was sent; nothing needs to happen.
  NoPendingTimeout,
  /// Return code `1`: a pending timeout was handled successfully, which for
  /// DTLS means OpenSSL retransmitted the last flight internally as a side
  /// effect of this call (this is not merely a status flag -- bytes may
  /// already be sitting in `ssl`'s write side by the time this returns).
  Retransmitted,
}

/// Safe wrapper for `DTLSv1_handle_timeout`. A return code of `-1`
/// ("a pending timeout event but it could not be handled successfully", per
/// the man page -- e.g. the maximum retransmission count was exceeded) is
/// surfaced as `Err(DtlsShimError::Openssl(..))`; per OpenSSL's own DTLS
/// documentation this is the caller's signal to tear the connection down,
/// not retry.
pub fn dtls_handle_timeout(ssl: &mut SslRef) -> Result<HandleTimeoutOutcome, DtlsShimError> {
  // SAFETY: same reasoning as `dtls_get_timeout` -- valid non-null `*mut SSL`
  // for the duration of the call, no pointer retained past it. The third
  // argument (`parg`) is documented as unused (`NULL`) for this control code.
  let ret = unsafe {
    openssl_sys::SSL_ctrl(
      ssl.as_ptr(),
      DTLS_CTRL_HANDLE_TIMEOUT,
      0,
      std::ptr::null_mut(),
    )
  };
  match ret {
    1 => Ok(HandleTimeoutOutcome::Retransmitted),
    0 => Ok(HandleTimeoutOutcome::NoPendingTimeout),
    _ => Err(DtlsShimError::Openssl(ErrorStack::get())),
  }
}

/// RAII wrapper around a `BIO_ADDR*`, freed on drop. Never exposes the raw
/// pointer outside this module.
struct BioAddr(NonNull<openssl_sys::BIO_ADDR>);

impl BioAddr {
  fn new() -> Result<Self, DtlsShimError> {
    // SAFETY: `BIO_ADDR_new` takes no arguments and either returns a valid
    // heap-allocated `BIO_ADDR*` or NULL on allocation failure; both
    // outcomes are handled below before the pointer is used for anything.
    let ptr = unsafe { raw::BIO_ADDR_new() };
    NonNull::new(ptr)
      .map(BioAddr)
      .ok_or_else(|| DtlsShimError::Openssl(ErrorStack::get()))
  }

  fn as_ptr(&self) -> *mut openssl_sys::BIO_ADDR {
    self.0.as_ptr()
  }

  /// Best-effort extraction into a `std::net::SocketAddr`. See the module
  /// notes on peer-address discovery for why this is `None` in the common
  /// case where `ssl`'s transport is rust-openssl's ordinary `Read + Write`
  /// bridge rather than a native datagram BIO -- that is expected, not a
  /// bug in this function.
  fn to_socket_addr(&self) -> Option<SocketAddr> {
    // SAFETY: `self.0` is a valid, live `BIO_ADDR*` for the lifetime of
    // `self` (freed only in `Drop`, which cannot run concurrently with an
    // outstanding `&self` borrow). `BIO_ADDR_family`/`_rawport` take a
    // `const BIO_ADDR*` and return plain integers, no output buffer to
    // get wrong.
    let family = unsafe { raw::BIO_ADDR_family(self.as_ptr()) };
    let port = u16::from_be(unsafe { raw::BIO_ADDR_rawport(self.as_ptr()) });

    match family {
      libc::AF_INET => {
        let mut buf = [0u8; 4];
        let mut len = 0usize;
        // SAFETY: `BIO_ADDR_rawaddress` writes a number of bytes dictated
        // solely by the `BIO_ADDR`'s address family -- it never reads
        // `*l` as a capacity, only reports the written length through it.
        // The write stays in bounds because this arm's `AF_INET` match
        // guarantees a 4-byte write and `buf` is pre-sized to exactly
        // that; the pre-sizing IS the bounds enforcement. The `len != 4`
        // check below is a consistency check on the reported length, not
        // what keeps the write in bounds.
        let ok = unsafe {
          raw::BIO_ADDR_rawaddress(self.as_ptr(), buf.as_mut_ptr() as *mut c_void, &mut len)
        };
        if ok == 0 || len != 4 {
          return None;
        }
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(buf)), port))
      }
      libc::AF_INET6 => {
        let mut buf = [0u8; 16];
        let mut len = 0usize;
        // SAFETY: same contract as the AF_INET arm -- the `AF_INET6`
        // match guarantees a 16-byte write into a buffer pre-sized to
        // exactly that.
        let ok = unsafe {
          raw::BIO_ADDR_rawaddress(self.as_ptr(), buf.as_mut_ptr() as *mut c_void, &mut len)
        };
        if ok == 0 || len != 16 {
          return None;
        }
        Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(buf)), port))
      }
      // AF_UNSPEC (the documented "BIO couldn't determine a peer"
      // outcome) or anything else this shim doesn't special-case.
      _ => None,
    }
  }
}

impl Drop for BioAddr {
  fn drop(&mut self) {
    // SAFETY: `self.0` was allocated by `BIO_ADDR_new` in `Self::new` and
    // has not been freed anywhere else -- `BioAddr` owns it exclusively
    // and this is the only `Drop` impl for it.
    unsafe { raw::BIO_ADDR_free(self.as_ptr()) }
  }
}

/// Outcome of [`dtlsv1_listen`], mirroring `DTLSv1_listen(3)`'s three-way
/// return-code contract (`>=1` / `0` / `<0`).
#[derive(Debug)]
pub enum ListenOutcome {
  /// Return code `0`: non-fatal. Per the man page this covers both
  /// "received a bare ClientHello with no cookie" (in which case OpenSSL
  /// has already queued a HelloVerifyRequest on `ssl`'s write side for the
  /// caller to flush out) and "received garbage/an invalid message" --
  /// this shim does not attempt to distinguish those, since the correct
  /// caller action is identical for both: flush any pending write-side
  /// bytes, then feed the next datagram from the same peer and call
  /// `dtlsv1_listen` again.
  Retry,
  /// Return code `>= 1`: a ClientHello with a verified cookie was received.
  /// `ssl` is now ready to continue via `SSL_accept`/`Ssl::accept`/
  /// `SslStream::accept`. `peer` is the address OpenSSL's BIO reported for
  /// the sender, if any -- see the module notes on peer-address discovery
  /// for why this is routinely `None` and must not be relied on as the
  /// caller's only source of the peer address.
  Accepted { peer: Option<SocketAddr> },
}

/// Safe wrapper for `DTLSv1_listen`. `ssl` must already have its read/write
/// BIOs configured (i.e. this is meant to be called on an `Ssl` created and
/// wired up the same way one would for `Ssl::accept`, before that accept
/// call), and `SSL_CTX_set_cookie_generate_cb`/`_verify_cb`
/// (`openssl::ssl::SslContextBuilder::set_cookie_generate_cb`/
/// `set_cookie_verify_cb`) must already be configured on its `SslContext` --
/// `DTLSv1_listen` requires both and this shim does not set them up on the
/// caller's behalf (see module-level "What this module deliberately does NOT
/// do").
///
/// Takes `&mut SslRef` even though `SslRef::as_ptr()` only needs `&self`:
/// the mutation happens on the C side of the FFI boundary where the borrow
/// checker cannot see it, and `SslRef` is `Sync` -- so the exclusive borrow
/// is the only thing in this signature preventing safe code from driving two
/// concurrent handshake operations on one `SSL *` (see the module notes on
/// exclusivity). Obtain one from an exclusive borrow of the owning stream
/// via [`ssl_mut`].
///
/// Returns `Err(DtlsShimError::UnsupportedOpenSslVersion { .. })` without
/// calling into OpenSSL at all if the linked library predates the ABI this
/// shim assumes (see the module notes on the runtime version gate). Returns
/// `Err(DtlsShimError::Openssl(..))` for `DTLSv1_listen`'s fatal
/// (`< 0`) return code.
pub fn dtlsv1_listen(ssl: &mut SslRef) -> Result<ListenOutcome, DtlsShimError> {
  if !dtls_listen_supported() {
    return Err(DtlsShimError::UnsupportedOpenSslVersion {
      linked: openssl::version::number() as i64,
      minimum: MIN_SUPPORTED_OPENSSL_VERSION,
    });
  }

  let addr = BioAddr::new()?;

  // SAFETY: `ssl.as_ptr()` is a valid, non-null `*mut SSL` for the duration
  // of this call. `addr.as_ptr()` is a valid, freshly-allocated, non-null
  // `BIO_ADDR*` that `DTLSv1_listen` is documented to fill in on success and
  // leave in an unspecified-but-still-valid-to-free state otherwise (it
  // never frees or reallocates the pointer itself, only writes through it) --
  // `addr`'s `Drop` frees it exactly once regardless of which branch below
  // is taken. `DTLSv1_listen` reads from and writes to `ssl`'s already-
  // configured BIOs; it does not take ownership of either pointer past the
  // call.
  let ret = unsafe { raw::DTLSv1_listen(ssl.as_ptr(), addr.as_ptr()) };

  if ret < 0 {
    return Err(DtlsShimError::Openssl(ErrorStack::get()));
  }
  if ret == 0 {
    return Ok(ListenOutcome::Retry);
  }
  Ok(ListenOutcome::Accepted {
    peer: addr.to_socket_addr(),
  })
}
