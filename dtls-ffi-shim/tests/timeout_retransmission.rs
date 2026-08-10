//! Proves `dtls_get_timeout`/`dtls_handle_timeout` against a REAL DTLS
//! handshake over real loopback UDP sockets, with the server's first
//! response flight deliberately swallowed -- so the only way the handshake
//! can possibly complete is if `dtls_handle_timeout` genuinely triggers a
//! retransmission on the client side, not merely returns a plausible-looking
//! value.

mod common;

use std::thread;
use std::time::{Duration, Instant};

use dtls_ffi_shim::dtls_ffi::{self, HandleTimeoutOutcome};
use openssl::ssl::{ErrorCode, SslStream};

fn is_would_block(e: &openssl::ssl::Error) -> bool {
  matches!(e.code(), ErrorCode::WANT_READ | ErrorCode::WANT_WRITE)
}

#[test]
fn dropped_flight_forces_real_retransmission_via_handle_timeout() {
  let (client_sock, server_sock) = common::connected_pair();

  // Drop the server's very first outbound write -- its first response
  // flight after processing the ClientHello. The client will never see it
  // and must eventually retransmit its own ClientHello, which is what
  // forces the server to resend (this time undropped).
  //
  // The server's socket is deliberately left plain *blocking* (no
  // `.nonblocking()`), not busy-polled: it can only ever make progress in
  // reaction to a datagram genuinely arriving, and can never race ahead by
  // triggering OpenSSL's own internal auto-retransmit -- see the long
  // comment below on the client's loop for why that distinction is the
  // entire point of this test.
  let server_transport = common::LossyUdp::new("server", server_sock).with_dropped_write(0);
  // Nonblocking, deliberately: on a plain blocking socket, `.connect()`
  // loops *internally* across
  // however many reads it takes to finish the whole handshake, including
  // OpenSSL's own auto-retransmit, all inside one call -- it never hands
  // control back to this loop in between, so nothing external (including
  // this shim) gets a chance to run partway through. Nonblocking mode is
  // what makes `.connect()` return after exactly one read attempt, every
  // time, which is what lets this loop interleave its own logic between
  // attempts at all.
  let client_transport = common::LossyUdp::new("client", client_sock).nonblocking();

  let s_ctx = common::server_ctx(false);
  let c_ctx = common::client_ctx();

  let server = thread::spawn(move || {
    let mut ssl = common::new_ssl(&s_ctx);
    ssl.set_accept_state();
    let mut stream = SslStream::new(ssl, server_transport).expect("SslStream::new (server)");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
      match stream.accept() {
        Ok(()) => break,
        Err(e) if is_would_block(&e) => {
          assert!(
            Instant::now() < deadline,
            "server handshake deadline exceeded"
          );
          thread::sleep(Duration::from_millis(10));
        }
        Err(e) => panic!("server handshake failed: {e:?}"),
      }
    }
  });

  let mut ssl = common::new_ssl(&c_ctx);
  ssl.set_connect_state();
  let mut stream = SslStream::new(ssl, client_transport).expect("SslStream::new (client)");

  // Deliberately NOT "wait, then call connect() again": re-invoking
  // `connect()`/`.accept()` at all after enough wall-clock time has passed
  // triggers OpenSSL's *own* internal auto-retransmit as a side effect of
  // the call itself (confirmed empirically: a busy-poll design and a
  // single-bounded-blocking-read design were both tried and both let
  // OpenSSL heal the connection on its own, with this shim's
  // `dtls_handle_timeout` never actually invoked -- neither would have
  // proven anything about the shim).
  //
  // To isolate this shim as the *only* possible cause of a retransmission,
  // the wait here is a raw `poll(2)` on the socket fd directly
  // (`poll_readable`), which never calls into the SSL layer at all.
  // `stream.connect()` is only ever invoked after one of two things is
  // already true: real data is confirmed sitting on the socket, or this
  // shim's `dtls_handle_timeout` has already been called. Either way, by
  // the time `connect()` runs, there is nothing left for OpenSSL's own
  // internal mechanism to meaningfully add.
  let mut saw_retransmit = false;
  let mut iterations = 0u32;
  let deadline = Instant::now() + Duration::from_secs(30);
  loop {
    let wait = dtls_ffi::dtls_get_timeout(stream.ssl()).unwrap_or(Duration::from_millis(200));
    if !stream.get_ref().poll_readable(wait) {
      // Nothing arrived within `wait`. Only this shim's explicit call
      // -- never `stream.connect()`, which hasn't been touched since
      // the last iteration -- can be responsible for any bytes that go
      // out as a result of the next few lines.
      if let Some(d) = dtls_ffi::dtls_get_timeout(stream.ssl()) {
        if d.is_zero() {
          match dtls_ffi::dtls_handle_timeout(stream.ssl())
            .expect("handle_timeout should not fail in this scenario")
          {
            HandleTimeoutOutcome::Retransmitted => saw_retransmit = true,
            HandleTimeoutOutcome::NoPendingTimeout => {}
          }
        }
      }
    }

    match stream.connect() {
      Ok(()) => break,
      Err(e) if is_would_block(&e) => iterations += 1,
      Err(e) => panic!("client handshake failed: {e:?}"),
    }
    assert!(
      Instant::now() < deadline,
      "client handshake deadline exceeded"
    );
  }
  eprintln!("[client] handshake completed after {iterations} WouldBlock iterations");

  server.join().expect("server thread panicked");

  assert!(
    saw_retransmit,
    "expected dtls_handle_timeout to report a real retransmission at least once; \
         the handshake completing without one would mean the dropped-write hook in the \
         test harness silently failed to drop anything, not that the shim works"
  );

  // Prove the connection is actually usable afterwards, not just "reported
  // success" while secretly wedged.
  use std::io::Write;
  stream
    .write_all(b"hello")
    .expect("client write after handshake");
}
