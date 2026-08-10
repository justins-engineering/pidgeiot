//! `dtlsv1_listen` sits directly on the pre-authentication attack surface:
//! bytes from any unauthenticated UDP sender reach it before any crypto
//! identity exists. These tests feed it garbage and truncated input and
//! assert only that (a) the process never panics/aborts and (b) garbage is
//! never classified as `Accepted`. A test process segfaulting or panicking
//! IS the failure signal here, not an assertion -- if any of these bodies
//! trip a panic, the test harness reports it as a failure on its own.

mod common;

use std::io::{self, Read, Write};

use dtls_ffi_shim::dtls_ffi::{self, ListenOutcome};
use openssl::ssl::SslStream;

/// Hands a fixed byte sequence to a single `read()` call, then reports
/// `WouldBlock` forever after -- simulates "exactly these bytes arrived in
/// one UDP datagram, and nothing else ever will," without needing a real
/// socket. Every `write()` (e.g. an outbound HelloVerifyRequest OpenSSL
/// might queue) is silently accepted and discarded.
struct OneShotInput {
  data: Option<Vec<u8>>,
}

impl OneShotInput {
  fn new(data: Vec<u8>) -> Self {
    OneShotInput { data: Some(data) }
  }
}

impl Read for OneShotInput {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    match self.data.take() {
      Some(d) => {
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        Ok(n)
      }
      None => Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "no more datagrams",
      )),
    }
  }
}

impl Write for OneShotInput {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    Ok(buf.len())
  }
  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

/// Runs `dtlsv1_listen` once against `input` on a fresh server-role `Ssl`
/// with cookie callbacks configured, and returns whatever it produced without
/// panicking (a panic inside this function fails the calling test on its
/// own, which is exactly the property under test).
fn try_listen_once(input: Vec<u8>) -> Result<ListenOutcome, dtls_ffi::DtlsShimError> {
  let ctx = common::server_ctx(true);
  let mut ssl = common::new_ssl(&ctx);
  ssl.set_accept_state();
  let stream = SslStream::new(ssl, OneShotInput::new(input)).expect("SslStream::new");
  dtls_ffi::dtlsv1_listen(stream.ssl())
}

#[test]
fn empty_datagram_does_not_panic_and_is_not_accepted() {
  let outcome = try_listen_once(Vec::new());
  assert!(
    !matches!(outcome, Ok(ListenOutcome::Accepted { .. })),
    "an empty datagram must never be treated as a verified ClientHello"
  );
}

#[test]
fn random_garbage_does_not_panic_and_is_not_accepted() {
  // Small deterministic xorshift PRNG -- avoids pulling in a `rand`
  // dependency just for test fixtures, while still exercising varied,
  // non-hand-picked byte patterns across lengths that span "shorter than
  // any real DTLS record header" through "plausible datagram size."
  let mut state: u32 = 0x9E3779B9;
  let mut next = move || {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
  };

  for len in [1usize, 2, 5, 13, 32, 64, 128, 256, 512] {
    let bytes: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
    let outcome = try_listen_once(bytes.clone());
    assert!(
      !matches!(outcome, Ok(ListenOutcome::Accepted { .. })),
      "random {len}-byte garbage must never be accepted as a verified ClientHello \
             (bytes: {bytes:02x?})"
    );
  }
}

#[test]
fn truncated_client_hello_does_not_panic_and_is_not_accepted() {
  // A real DTLS ClientHello's record header (RFC 6347 section 4.1): content
  // type 22 (handshake), version 0xFEFD (DTLS 1.2), a 16-bit epoch, a
  // 48-bit sequence number, then a 16-bit length. Build a syntactically
  // plausible header claiming a body far longer than what's actually
  // supplied, which is exactly the "fragmented/incomplete message"
  // scenario DTLSv1_listen(3) documents it cannot handle statelessly (it
  // only supports ClientHellos that fit in a single datagram) -- proving
  // that documented limitation fails closed (Retry/Err) rather than open
  // (a crash, or worse, `Accepted`).
  let mut record = vec![
    22, // content type: handshake
    0xFE, 0xFD, // DTLS 1.2
    0x00, 0x00, // epoch
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, // sequence number
    0x00, 0x40, // length: claims 64 bytes of handshake body follows
  ];
  // ...but only supply 8 actual body bytes.
  record.extend_from_slice(&[1, 0, 0, 4, 0, 0, 0, 0]);

  let outcome = try_listen_once(record);
  assert!(
    !matches!(outcome, Ok(ListenOutcome::Accepted { .. })),
    "a truncated ClientHello record must never be accepted"
  );
}

#[test]
fn oversized_single_datagram_does_not_panic() {
  // Not a real ClientHello at all, just a large buffer of a fixed byte --
  // makes sure nothing in the shim's path assumes an upper bound on datagram
  // size that it doesn't actually enforce (any such bound belongs to the
  // caller's socket-read layer, not this shim, but the shim still must not
  // misbehave if handed a large buffer).
  let bytes = vec![0x41u8; 65_507]; // max theoretical UDP payload size
  let outcome = try_listen_once(bytes);
  assert!(!matches!(outcome, Ok(ListenOutcome::Accepted { .. })));
}
