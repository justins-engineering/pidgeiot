//! Flight retransmission through the timer-callback idiom: the transport
//! surfaces `TimerExpired` from the same timer state mbedTLS drives, and
//! mbedTLS resends the lost flight itself. This is the behavior that only
//! shows up under loss, ported from the OpenSSL shim's
//! `timeout_retransmission` shape.

mod common;

use common::*;
use mbedtls_ffi_shim::{Config, Session};
use std::sync::Arc;

#[test]
fn lost_server_flight_is_retransmitted_via_timer() {
  let (cli_io, mut srv_io) = io_pair();
  // Swallow the server's flight-2 datagram (send #1; #0 is the HVR), so
  // only a timer-driven retransmission can complete this handshake.
  srv_io.drop_sends = vec![1];

  let cli_conf = client_config(TEST_IDENTITY, TEST_PSK, true);
  let mut srv_conf = Config::server(accepting_resolver()).expect("server config");
  // Milliseconds instead of the protocol-default seconds, purely so the
  // test's forced loss resolves quickly.
  srv_conf.set_handshake_timeout(50, 400);
  let srv_conf = Arc::new(srv_conf);

  let mut client = Session::new(&cli_conf, cli_io).expect("client session");
  client.offer_zero_length_cid().expect("cid offer");
  let mut server = Session::new(&srv_conf, srv_io).expect("server session");
  let cid = fresh_cid();
  arm_server(&mut server, &cid);

  let result = drive(&mut client, &mut server, &cid);
  assert!(
    result.client_done && result.server_done,
    "handshake must survive a lost server flight (client fail: {:?}, server fail: {:?})",
    result.client_failure,
    result.server_failure
  );

  // The swallowed flight plus at least one retransmission of it: compare
  // the handshake record payload prefixes of send #1 and a later send.
  let sent = &server.io().sent;
  assert!(
    sent.len() >= 3,
    "expected HVR + flight 2 + a retransmission, saw {} sends",
    sent.len()
  );
  let lost = &sent[1];
  assert!(
    sent[2..].iter().any(|d| d == lost || d[0] == lost[0]),
    "no retransmission of the lost flight observed"
  );
}

#[test]
fn client_retransmits_into_a_silent_server_and_gives_up_cleanly() {
  // A client whose every datagram is dropped must neither wedge nor
  // panic; with a short backoff it fails in bounded time.
  let (mut cli_io, _srv_io) = io_pair();
  cli_io.drop_sends = (0..64).collect();
  let mut cli_conf = Config::client(TEST_IDENTITY, TEST_PSK, false).expect("client config");
  cli_conf.set_handshake_timeout(10, 50);
  let cli_conf = Arc::new(cli_conf);
  let mut client = Session::new(&cli_conf, cli_io).expect("client session");

  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
  loop {
    match client.handshake() {
      mbedtls_ffi_shim::HandshakeStatus::Done => panic!("cannot complete against silence"),
      mbedtls_ffi_shim::HandshakeStatus::Failed(_) => break,
      _ => {
        assert!(
          std::time::Instant::now() < deadline,
          "client never gave up on a dead path"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
      }
    }
  }
  assert!(
    client.io().send_count >= 2,
    "retransmissions were attempted"
  );
}
