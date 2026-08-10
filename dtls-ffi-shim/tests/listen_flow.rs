//! Proves `dtlsv1_listen` against a REAL cookie-exchange round trip over
//! loopback UDP: a real DTLS client (which automatically resends its
//! ClientHello with a cookie on receiving a HelloVerifyRequest -- standard
//! client-side DTLS behavior, no special code needed) against a server driven
//! entirely through this shim's `dtlsv1_listen` until it reports `Accepted`,
//! then completed via the ordinary `SslStream::accept`.

mod common;

use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use dtls_ffi_shim::dtls_ffi::{self, ListenOutcome};
use openssl::ssl::{ErrorCode, SslStream};

fn is_would_block(e: &openssl::ssl::Error) -> bool {
  matches!(e.code(), ErrorCode::WANT_READ | ErrorCode::WANT_WRITE)
}

#[test]
fn cookie_exchange_via_dtlsv1_listen_then_completes_handshake() {
  let (client_sock, server_sock) = common::connected_pair();
  let server_transport = common::LossyUdp::new("server", server_sock).nonblocking();
  let client_transport = common::LossyUdp::new("client", client_sock).nonblocking();

  let s_ctx = common::server_ctx(true); // cookie gen/verify configured
  let c_ctx = common::client_ctx();

  let server = thread::spawn(move || {
    let mut ssl = common::new_ssl(&s_ctx);
    ssl.set_accept_state();
    let mut stream = SslStream::new(ssl, server_transport).expect("SslStream::new (server)");

    let deadline = Instant::now() + Duration::from_secs(30);

    // Drive DTLSv1_listen until it reports a verified ClientHello. This
    // is the actual pre-handshake, pre-allocation path -- no per-client
    // state exists yet while we're in this loop.
    let peer = loop {
      match dtls_ffi::dtlsv1_listen(dtls_ffi::ssl_mut(&mut stream)) {
        Ok(ListenOutcome::Accepted { peer }) => break peer,
        Ok(ListenOutcome::Retry) => {
          assert!(Instant::now() < deadline, "listen deadline exceeded");
          thread::sleep(Duration::from_millis(10));
        }
        Err(e) => panic!("dtlsv1_listen failed: {e}"),
      }
    };

    // Documented (and now empirically pinned-down) behavior: rust-openssl's
    // generic Read+Write transport bridge cannot report a peer address
    // back through BIO_ADDR, unlike a native BIO_s_datagram. See the
    // peer-address discovery notes in dtls_ffi.rs. If this ever starts
    // returning `Some`, the module docs describing today's real behavior
    // need updating.
    assert_eq!(
      peer, None,
      "peer discovery through the Read+Write bridge is expected to be unavailable; \
             if this now returns Some(_), the SAFETY docs in dtls_ffi.rs need updating"
    );

    // Now complete the handshake from where DTLSv1_listen left the SSL
    // state machine -- exactly as DTLSv1_listen(3) documents ("typically
    // ... continue the handshake ... via SSL_accept()").
    loop {
      match stream.accept() {
        Ok(()) => break,
        Err(e) if is_would_block(&e) => {
          assert!(Instant::now() < deadline, "server accept deadline exceeded");
          thread::sleep(Duration::from_millis(10));
        }
        Err(e) => panic!("server accept failed after listen: {e:?}"),
      }
    }

    // Prove the resulting connection actually carries data, round-tripped
    // through the exact Ssl object dtlsv1_listen operated on. Nonblocking
    // read, so retry through WouldBlock the same as the handshake loops
    // above (accumulating into a Vec rather than using read_exact, which
    // doesn't preserve partial progress across separate nonblocking
    // calls) -- the client hasn't necessarily written yet by the time we
    // get here.
    let mut got = Vec::new();
    let mut chunk = [0u8; 5];
    while got.len() < 5 {
      match stream.read(&mut chunk) {
        Ok(0) => panic!("server read: unexpected EOF"),
        Ok(n) => got.extend_from_slice(&chunk[..n]),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
          assert!(Instant::now() < deadline, "server read deadline exceeded");
          thread::sleep(Duration::from_millis(10));
        }
        Err(e) => panic!("server read after listen+accept failed: {e:?}"),
      }
    }
    assert_eq!(&got, b"hello");
  });

  let mut ssl = common::new_ssl(&c_ctx);
  ssl.set_connect_state();
  let mut stream = SslStream::new(ssl, client_transport).expect("SslStream::new (client)");

  let deadline = Instant::now() + Duration::from_secs(30);
  loop {
    match stream.connect() {
      Ok(()) => break,
      Err(e) if is_would_block(&e) => {
        assert!(
          Instant::now() < deadline,
          "client connect deadline exceeded"
        );
        thread::sleep(Duration::from_millis(10));
      }
      Err(e) => panic!("client connect failed: {e:?}"),
    }
  }
  stream.write_all(b"hello").expect("client write");

  server.join().expect("server thread panicked");
}
