//! Shared test harness: a real loopback UDP transport wrapped as `Read +
//! Write` (the same shape rust-openssl's own `Ssl::connect`/`Ssl::accept`
//! expect), with a hook to deliberately swallow specific outbound datagrams
//! so tests can force real retransmission rather than merely asserting about
//! it.
//!
//! `allow(dead_code)`: this module is compiled fresh into each `tests/*.rs`
//! binary (Rust's per-test-file integration test model), and no single test
//! file uses every helper here -- the "unused" warnings are per-binary
//! artifacts, not a real dead-code problem in the shared module.
#![allow(dead_code)]

use std::io::{self, Read, Write};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use openssl::ssl::{Ssl, SslContext, SslMethod, SslOptions, SslVerifyMode};

fn t0() -> Instant {
  static T0: OnceLock<Instant> = OnceLock::new();
  *T0.get_or_init(Instant::now)
}

/// A connected UDP socket as `Read + Write`, with an optional "drop the Nth
/// write call" hook used to simulate a lost datagram.
pub struct LossyUdp {
  label: &'static str,
  sock: UdpSocket,
  write_count: Arc<AtomicUsize>,
  /// 0-based index of the write() call to silently swallow (report success
  /// to the SSL layer without actually sending), if any.
  drop_write_index: Option<usize>,
}

impl LossyUdp {
  /// Real blocking socket by default -- correct for a side of the test
  /// harness that should only ever progress in reaction to genuinely
  /// arrived data, never on its own busy-polling. See `.nonblocking()` for
  /// the side that needs to drive its own explicit retry/timeout logic
  /// (nonblocking is what lets that side call `dtls_get_timeout`/
  /// `dtls_handle_timeout` deliberately instead of OpenSSL's own internal
  /// auto-retransmit-on-reinvocation taking over -- see the long comment in
  /// `timeout_retransmission.rs` for why that distinction turned out to
  /// matter for this specific test).
  pub fn new(label: &'static str, sock: UdpSocket) -> Self {
    LossyUdp {
      label,
      sock,
      write_count: Arc::new(AtomicUsize::new(0)),
      drop_write_index: None,
    }
  }

  pub fn nonblocking(self) -> Self {
    self.sock.set_nonblocking(true).expect("set_nonblocking");
    self
  }

  /// Bounds the next blocking read by `d` (like a real event loop bounding
  /// its `epoll_wait`/`select` by "time until my own timer is next due") --
  /// wakes up on real data OR the bound elapsing, whichever is first,
  /// rather than a fixed busy-poll interval that risks either spinning or
  /// (worse) oversleeping past data that already arrived.
  pub fn set_read_timeout(&self, d: Option<std::time::Duration>) {
    self.sock.set_read_timeout(d).expect("set_read_timeout");
  }

  /// True if a read from this socket wouldn't block right now, checked via
  /// a raw `poll(2)` on the socket's fd directly -- crucially, this does
  /// NOT go anywhere near the SSL layer (no `Ssl`/`SslStream` method is
  /// invoked), so unlike calling `.connect()`/`.accept()`/`.read()` again,
  /// it cannot itself trigger OpenSSL's own internal
  /// auto-retransmit-on-reinvocation. Used to isolate this shim's
  /// `dtls_handle_timeout` as the *only* thing that can have caused a
  /// retransmission observed in a test -- see the long comment in
  /// `timeout_retransmission.rs`.
  pub fn poll_readable(&self, timeout: std::time::Duration) -> bool {
    use std::os::unix::io::AsRawFd;
    let mut pfd = libc::pollfd {
      fd: self.sock.as_raw_fd(),
      events: libc::POLLIN,
      revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: `pfd` is a single, stack-local, correctly-initialized
    // `pollfd` and `1` matches the array length passed in `&mut pfd`
    // (poll(2) treats the pointer as a 1-element array here).
    let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    ret > 0 && (pfd.revents & libc::POLLIN) != 0
  }

  pub fn with_dropped_write(mut self, index: usize) -> Self {
    self.drop_write_index = Some(index);
    self
  }

  pub fn write_count_handle(&self) -> Arc<AtomicUsize> {
    self.write_count.clone()
  }
}

impl Read for LossyUdp {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    self.sock.recv(buf)
  }
}

impl Write for LossyUdp {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    let n = self.write_count.fetch_add(1, Ordering::SeqCst);
    let dropped = self.drop_write_index == Some(n);
    eprintln!(
      "[{:>7.3}s] [{}] write #{n}, {} bytes, drop={dropped}",
      t0().elapsed().as_secs_f64(),
      self.label,
      buf.len(),
    );
    if dropped {
      // Pretend it went out. The peer never sees these bytes.
      return Ok(buf.len());
    }
    self.sock.send(buf)
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

/// Binds two loopback UDP sockets and connects them to each other.
pub fn connected_pair() -> (UdpSocket, UdpSocket) {
  let a = UdpSocket::bind("127.0.0.1:0").expect("bind a");
  let b = UdpSocket::bind("127.0.0.1:0").expect("bind b");
  a.connect(b.local_addr().unwrap()).expect("connect a->b");
  b.connect(a.local_addr().unwrap()).expect("connect b->a");
  (a, b)
}

const TEST_PSK: &[u8] = b"handoff-test-psk-do-not-use-in-prod";
const TEST_PSK_IDENTITY: &[u8] = b"loft-test-client";

/// A minimal PSK DTLS client `SslContext` -- no certificates needed, matches
/// the actual PSK-based device-auth model this shim is being built for.
pub fn client_ctx() -> SslContext {
  let mut b = SslContext::builder(SslMethod::dtls()).expect("client ctx builder");
  b.set_cipher_list("PSK-AES128-GCM-SHA256")
    .expect("cipher list");
  b.set_psk_client_callback(|_ssl, _hint, identity, psk| {
    identity[..TEST_PSK_IDENTITY.len()].copy_from_slice(TEST_PSK_IDENTITY);
    identity[TEST_PSK_IDENTITY.len()] = 0;
    psk[..TEST_PSK.len()].copy_from_slice(TEST_PSK);
    Ok(TEST_PSK.len())
  });
  b.set_verify(SslVerifyMode::NONE);
  b.build()
}

/// A minimal PSK DTLS server `SslContext`. `with_cookie` additionally wires
/// up `SSL_OP_COOKIE_EXCHANGE` and cookie generate/verify callbacks, the
/// prerequisite `DTLSv1_listen(3)` documents.
pub fn server_ctx(with_cookie: bool) -> SslContext {
  let mut b = SslContext::builder(SslMethod::dtls()).expect("server ctx builder");
  b.set_cipher_list("PSK-AES128-GCM-SHA256")
    .expect("cipher list");
  b.set_psk_server_callback(|_ssl, identity, psk| {
    if identity == Some(TEST_PSK_IDENTITY) {
      psk[..TEST_PSK.len()].copy_from_slice(TEST_PSK);
      Ok(TEST_PSK.len())
    } else {
      Ok(0)
    }
  });
  b.set_verify(SslVerifyMode::NONE);

  if with_cookie {
    b.set_options(SslOptions::COOKIE_EXCHANGE);
    b.set_cookie_generate_cb(|_ssl, cookie| {
      let secret = b"test-cookie-secret-fixed-for-determinism";
      cookie[..secret.len()].copy_from_slice(secret);
      Ok(secret.len())
    });
    b.set_cookie_verify_cb(|_ssl, cookie| {
      let secret = b"test-cookie-secret-fixed-for-determinism";
      cookie == secret
    });
  }

  b.build()
}

pub fn new_ssl(ctx: &SslContext) -> Ssl {
  Ssl::new(ctx).expect("Ssl::new")
}
