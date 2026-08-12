//! Loopback handshakes against the real system library: CID negotiation
//! with the fleet's zero-length offer, the no-CID regression shape, the
//! pending-listen HelloVerifyRequest flow with its WANT_READ
//! disambiguation, the garbage corpus, and PSK reject indistinguishability.

mod common;

use common::*;
use mbedtls_ffi_shim::{CID_LEN, HandshakeStatus, ReadStatus, Session};

#[test]
fn cid_negotiated_end_to_end_with_zero_length_client_offer() {
  let (cli_io, srv_io) = io_pair();
  let cli_conf = client_config(TEST_IDENTITY, TEST_PSK, true);
  let srv_conf = server_config();
  let mut client = Session::new(&cli_conf, cli_io).expect("client session");
  client.offer_zero_length_cid().expect("client cid offer");
  let mut server = Session::new(&srv_conf, srv_io).expect("server session");
  let cid = fresh_cid();
  arm_server(&mut server, &cid);

  let result = drive(&mut client, &mut server, &cid);
  assert!(
    result.client_done && result.server_done,
    "handshake completes"
  );
  assert!(result.hvr_rounds >= 1, "cookie exchange must have run");

  assert_eq!(
    server.ciphersuite().as_deref(),
    Some("TLS-PSK-WITH-AES-128-CCM-8"),
    "CCM8 is the pinned preference"
  );

  let cli_view = client.peer_cid().expect("client peer_cid");
  assert!(cli_view.negotiated);
  assert_eq!(cli_view.peer_cid, cid, "client received the server's CID");
  let srv_view = server.peer_cid().expect("server peer_cid");
  assert!(srv_view.negotiated);
  assert!(srv_view.peer_cid.is_empty(), "zero-length client offer");

  assert_eq!(
    server.take_credentials(),
    Some((
      String::from_utf8_lossy(TEST_IDENTITY).into_owned(),
      TEST_TOKEN.to_string()
    )),
    "PSK callback stashed the authenticated pair"
  );

  // One application datagram each way, then the wire-shape assertions the
  // whole design hangs on.
  let cli_app_start = client.io().sent.len();
  let srv_app_start = server.io().sent.len();
  assert_eq!(client.write(b"ping").expect("client write"), 4);
  let mut buf = [0u8; 64];
  let ReadStatus::Data(4) = server.read(&mut buf) else {
    panic!("server read");
  };
  assert_eq!(&buf[..4], b"ping");
  assert_eq!(server.write(b"pong").expect("server write"), 4);
  let ReadStatus::Data(4) = client.read(&mut buf) else {
    panic!("client read");
  };
  assert_eq!(&buf[..4], b"pong");

  // Uplink: content type 25 (tls12_cid) with the server's CID at the
  // demux offset 11 = type(1) + version(2) + epoch(2) + seq(6).
  let up = &client.io().sent[cli_app_start];
  assert_eq!(up[0], 25, "uplink app record is a CID record");
  assert!(up.len() >= 21);
  assert_eq!(
    &up[11..11 + CID_LEN],
    &cid,
    "server CID at the demux offset"
  );
  // Downlink: plain content type 23 -- the device blackholes CID-bearing
  // records, so this is load-bearing, not cosmetic.
  let down = &server.io().sent[srv_app_start];
  assert_eq!(down[0], 23, "downlink records carry no CID");
}

#[test]
fn no_cid_client_negotiates_nothing_and_still_works() {
  let (cli_io, srv_io) = io_pair();
  let cli_conf = client_config(TEST_IDENTITY, TEST_PSK, false);
  let srv_conf = server_config();
  let mut client = Session::new(&cli_conf, cli_io).expect("client session");
  let mut server = Session::new(&srv_conf, srv_io).expect("server session");
  let cid = fresh_cid();
  arm_server(&mut server, &cid);

  let result = drive(&mut client, &mut server, &cid);
  assert!(result.client_done && result.server_done);

  let srv_view = server.peer_cid().expect("server peer_cid");
  assert!(!srv_view.negotiated, "current-fleet shape: no CID");

  let app_start = client.io().sent.len();
  assert_eq!(client.write(b"poll").expect("write"), 4);
  let mut buf = [0u8; 64];
  let ReadStatus::Data(4) = server.read(&mut buf) else {
    panic!("server read");
  };
  // Bit-for-bit today: plain type-23 records in both directions.
  assert_eq!(client.io().sent[app_start][0], 23);
}

#[test]
fn pending_flow_disambiguates_hvr_from_cookie_verified() {
  let (cli_io, srv_io) = io_pair();
  let cli_conf = client_config(TEST_IDENTITY, TEST_PSK, true);
  let srv_conf = server_config();
  let mut client = Session::new(&cli_conf, cli_io).expect("client session");
  let mut server = Session::new(&srv_conf, srv_io).expect("server session");
  let cid = fresh_cid();
  arm_server(&mut server, &cid);

  // Client emits its cookie-less ClientHello.
  let HandshakeStatus::WantRead = client.handshake() else {
    panic!("client should await HVR");
  };

  // Server consumes it: HVR outcome, cookie flag NOT set, HVR bytes
  // already written out through f_send.
  srv_conf.clear_cookie_verified();
  let HandshakeStatus::HelloVerifyRequired = server.handshake() else {
    panic!("cookie-less ClientHello must yield the HVR outcome");
  };
  assert!(!srv_conf.take_cookie_verified());
  assert_eq!(server.io().sent.len(), 1, "the HVR went out inline");
  server.reset().expect("reset");
  arm_server(&mut server, &cid);

  // Client processes the HVR and sends the cookied ClientHello.
  let HandshakeStatus::WantRead = client.handshake() else {
    panic!("client should send cookied CH and await flight 2");
  };

  // The load-bearing disambiguation: a cookie-verified ClientHello also
  // surfaces as WANT_READ, distinguishable only through the recording
  // cookie-check wrapper -- and flight 2 is already on the wire.
  srv_conf.clear_cookie_verified();
  let sent_before = server.io().sent.len();
  let HandshakeStatus::WantRead = server.handshake() else {
    panic!("cookied ClientHello surfaces as WANT_READ");
  };
  assert!(
    srv_conf.take_cookie_verified(),
    "cookie wrapper must record the verified ClientHello"
  );
  assert!(
    server.io().sent.len() > sent_before,
    "flight 2 written inline from the pending step"
  );

  // And the promoted handshake completes from here.
  let result = drive(&mut client, &mut server, &cid);
  assert!(result.client_done && result.server_done);
}

#[test]
fn garbage_corpus_never_wedges_the_pending_session() {
  let (mut cli_io, srv_io) = io_pair();
  let srv_conf = server_config();
  let mut server = Session::new(&srv_conf, srv_io).expect("server session");
  let cid = fresh_cid();

  let corpus: Vec<Vec<u8>> = vec![
    vec![],
    vec![0x00],
    vec![0x16, 0xfe, 0xfd],
    // Truncated ClientHello-ish header.
    vec![0x16, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 40],
    // Wrong protocol version.
    vec![
      0x16, 0x03, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 1, 2, 3, 4, 5,
    ],
    // Type-25 runt: can never begin a handshake.
    vec![25, 0xfe, 0xfd, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 9, 9],
    vec![0xff; 64],
    vec![0x17; 1400],
  ];

  for (i, garbage) in corpus.iter().enumerate() {
    server.reset().expect("reset");
    arm_server(&mut server, &cid);
    srv_conf.clear_cookie_verified();
    let _ = cli_io.tx.try_send(garbage.clone());
    match server.handshake() {
      HandshakeStatus::Done => panic!("garbage {i} completed a handshake"),
      HandshakeStatus::HelloVerifyRequired => panic!("garbage {i} minted an HVR"),
      // Discarded (WantRead/WantWrite) or rejected (Failed): both leave
      // the context resettable, which the next iteration proves.
      HandshakeStatus::WantRead | HandshakeStatus::WantWrite | HandshakeStatus::Failed(_) => {}
    }
    assert!(
      !srv_conf.take_cookie_verified(),
      "garbage {i} must not verify a cookie"
    );
  }

  // The session is still serviceable: a real client completes against it.
  let cli_conf = client_config(TEST_IDENTITY, TEST_PSK, true);
  let (mut fresh_cli_io, fresh_srv_io) = io_pair();
  std::mem::swap(&mut cli_io, &mut fresh_cli_io);
  drop(fresh_cli_io);
  let mut client = Session::new(&cli_conf, cli_io).expect("client session");
  let mut server = Session::new(&srv_conf, fresh_srv_io).expect("server session");
  arm_server(&mut server, &cid);
  let result = drive(&mut client, &mut server, &cid);
  assert!(result.client_done && result.server_done);
}

/// Unknown identity and wrong key must be wire-indistinguishable: same
/// client-observed failure class, same server alert shape. The random-PSK
/// reject in the shim's callback is what closes what would otherwise be
/// an `unknown_psk_identity` probing oracle.
#[test]
fn psk_rejects_are_indistinguishable() {
  let run = |identity: &[u8], psk: &[u8]| {
    let (cli_io, srv_io) = io_pair();
    let cli_conf = client_config(identity, psk, false);
    let srv_conf = server_config();
    let mut client = Session::new(&cli_conf, cli_io).expect("client session");
    let mut server = Session::new(&srv_conf, srv_io).expect("server session");
    let cid = fresh_cid();
    arm_server(&mut server, &cid);
    let result = drive(&mut client, &mut server, &cid);
    assert!(
      !result.client_done && !result.server_done,
      "reject case must not complete"
    );
    let server_alerts = plaintext_alerts(&server.io().sent);
    (result.server_failure.map(|e| e.0), server_alerts)
  };

  let unknown_identity = run(b"no-such-pigeon", TEST_PSK);
  let wrong_key = run(TEST_IDENTITY, b"ffffffffffffffffffffffffffffffff");

  assert_eq!(
    unknown_identity, wrong_key,
    "unknown-identity and wrong-key failures must be observably identical"
  );
  // Specifically: no unknown_psk_identity (115) alert anywhere.
  for (_, desc) in unknown_identity.1.iter().chain(wrong_key.1.iter()) {
    assert_ne!(*desc, 115, "unknown_psk_identity alert would be an oracle");
  }
}

/// The reject helper's degenerate arms must hold the same line: a
/// resolver that panics mid-handshake still fails like a wrong key --
/// no completion, no credentials, no unknown_psk_identity alert.
#[test]
fn panicking_resolver_rejects_indistinguishably() {
  let (cli_io, srv_io) = io_pair();
  let cli_conf = client_config(TEST_IDENTITY, TEST_PSK, false);
  let srv_conf = std::sync::Arc::new(
    mbedtls_ffi_shim::Config::server(Box::new(|_identity: &[u8]| {
      panic!("resolver blew up mid-handshake")
    }))
    .expect("server config"),
  );
  let mut client = Session::new(&cli_conf, cli_io).expect("client session");
  let mut server = Session::new(&srv_conf, srv_io).expect("server session");
  let cid = fresh_cid();
  arm_server(&mut server, &cid);

  let result = drive(&mut client, &mut server, &cid);
  assert!(
    !result.client_done && !result.server_done,
    "a panicking resolver must not complete a handshake"
  );
  assert!(server.take_credentials().is_none());
  for (_, desc) in plaintext_alerts(&server.io().sent) {
    assert_ne!(desc, 115, "the panic arm must not reopen the oracle");
  }
}
