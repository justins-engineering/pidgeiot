//! Shared test harness: an in-memory datagram channel as the shim's
//! `MbedIo`, with outbound capture and selective datagram loss, plus a
//! single-thread alternate-stepping handshake driver that runs the
//! server's pending-listen flow (HelloVerifyRequest, reset, re-apply
//! transport id and CID) the same way loft's listener does.
//!
//! `allow(dead_code)`: compiled fresh into each `tests/*.rs` binary; no
//! single test file uses every helper.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::time::Duration;

use mbedtls_ffi_shim::{
  CID_LEN, Config, HandshakeStatus, MbedIo, PskCallback, RecvOutcome, ResolvedPsk, SendOutcome,
  Session, TimerState,
};

pub const TEST_IDENTITY: &[u8] = b"shim-test-pigeon";
pub const TEST_PSK: &[u8] = b"0123456789abcdef0123456789abcdef";
pub const TEST_TOKEN: &str = "shim-test-token";
pub const TRANSPORT_ID: &[u8] = b"192.0.2.1:5684";

/// In-memory datagram transport: nonblocking (the drivers alternate-step
/// both sides in one thread), captures everything sent, and can swallow
/// chosen send calls to force real retransmission.
pub struct ChanIo {
  pub rx: Receiver<Vec<u8>>,
  pub tx: SyncSender<Vec<u8>>,
  pub sent: Vec<Vec<u8>>,
  pub send_count: usize,
  pub drop_sends: Vec<usize>,
}

impl MbedIo for ChanIo {
  fn send(&mut self, buf: &[u8]) -> SendOutcome {
    let n = self.send_count;
    self.send_count += 1;
    self.sent.push(buf.to_vec());
    if self.drop_sends.contains(&n) {
      // Pretend it went out; the peer never sees these bytes.
      return SendOutcome::Sent(buf.len());
    }
    match self.tx.try_send(buf.to_vec()) {
      Ok(()) => SendOutcome::Sent(buf.len()),
      Err(_) => SendOutcome::Failed,
    }
  }

  fn recv(&mut self, buf: &mut [u8], timer: &TimerState) -> RecvOutcome {
    match self.rx.try_recv() {
      Ok(d) => {
        if d.len() > buf.len() {
          // Whole-datagram drop, never truncation.
          return RecvOutcome::WantRead;
        }
        buf[..d.len()].copy_from_slice(&d);
        RecvOutcome::Data(d.len())
      }
      Err(TryRecvError::Empty) => {
        if timer.final_expired() {
          RecvOutcome::TimerExpired
        } else {
          RecvOutcome::WantRead
        }
      }
      Err(TryRecvError::Disconnected) => RecvOutcome::Closed,
    }
  }
}

pub fn io_pair() -> (ChanIo, ChanIo) {
  let (a_tx, a_rx) = sync_channel(64);
  let (b_tx, b_rx) = sync_channel(64);
  let a = ChanIo {
    rx: a_rx,
    tx: b_tx,
    sent: Vec::new(),
    send_count: 0,
    drop_sends: Vec::new(),
  };
  let b = ChanIo {
    rx: b_rx,
    tx: a_tx,
    sent: Vec::new(),
    send_count: 0,
    drop_sends: Vec::new(),
  };
  (a, b)
}

pub fn accepting_resolver() -> PskCallback {
  Box::new(|identity: &[u8]| {
    (identity == TEST_IDENTITY).then(|| ResolvedPsk {
      psk: TEST_PSK.to_vec(),
      identity: String::from_utf8_lossy(identity).into_owned(),
      token: TEST_TOKEN.to_string(),
    })
  })
}

pub fn server_config() -> Arc<Config> {
  Arc::new(Config::server(accepting_resolver()).expect("server config"))
}

pub fn client_config(identity: &[u8], psk: &[u8], offer_cid: bool) -> Arc<Config> {
  Arc::new(Config::client(identity, psk, offer_cid).expect("client config"))
}

/// Applies the per-attempt pending-listen state, mirroring loft's flow.
pub fn arm_server(server: &mut Session<ChanIo>, cid: &[u8; CID_LEN]) {
  server.set_mtu(1400);
  server
    .set_client_transport_id(TRANSPORT_ID)
    .expect("transport id");
  server.set_own_cid(cid).expect("own cid");
}

pub struct DriveResult {
  pub client_done: bool,
  pub server_done: bool,
  pub hvr_rounds: usize,
  pub client_failure: Option<mbedtls_ffi_shim::MbedError>,
  pub server_failure: Option<mbedtls_ffi_shim::MbedError>,
}

/// Alternate-steps both sides to completion or first failure, running the
/// server's HelloVerifyRequest reset flow, sleeping briefly only when
/// neither side can progress (which is what lets retransmission timers
/// actually fire under forced loss).
pub fn drive(
  client: &mut Session<ChanIo>,
  server: &mut Session<ChanIo>,
  server_cid: &[u8; CID_LEN],
) -> DriveResult {
  let mut result = DriveResult {
    client_done: false,
    server_done: false,
    hvr_rounds: 0,
    client_failure: None,
    server_failure: None,
  };
  for _ in 0..20_000 {
    if (result.client_done || result.client_failure.is_some())
      && (result.server_done || result.server_failure.is_some())
    {
      return result;
    }
    if result.client_failure.is_some() || result.server_failure.is_some() {
      // Give the survivor a bounded chance to observe the failure alert.
      for _ in 0..100 {
        if !result.client_done && result.client_failure.is_none() {
          match client.handshake() {
            HandshakeStatus::Done => result.client_done = true,
            HandshakeStatus::Failed(e) => result.client_failure = Some(e),
            _ => {}
          }
        }
        if !result.server_done && result.server_failure.is_none() {
          match server.handshake() {
            HandshakeStatus::Done => result.server_done = true,
            HandshakeStatus::Failed(e) => result.server_failure = Some(e),
            HandshakeStatus::HelloVerifyRequired => {}
            _ => {}
          }
        }
        std::thread::sleep(Duration::from_millis(1));
      }
      return result;
    }

    let mut progressed = false;
    if !result.client_done {
      match client.handshake() {
        HandshakeStatus::Done => {
          result.client_done = true;
          progressed = true;
        }
        HandshakeStatus::WantRead | HandshakeStatus::WantWrite => {}
        HandshakeStatus::HelloVerifyRequired => panic!("client got HVR status"),
        HandshakeStatus::Failed(e) => result.client_failure = Some(e),
      }
    }
    if !result.server_done {
      match server.handshake() {
        HandshakeStatus::Done => {
          result.server_done = true;
          progressed = true;
        }
        HandshakeStatus::WantRead | HandshakeStatus::WantWrite => {}
        HandshakeStatus::HelloVerifyRequired => {
          result.hvr_rounds += 1;
          server.reset().expect("server reset");
          arm_server(server, server_cid);
          progressed = true;
        }
        HandshakeStatus::Failed(e) => result.server_failure = Some(e),
      }
    }
    if !progressed {
      std::thread::sleep(Duration::from_millis(2));
    }
  }
  panic!("handshake made no terminal progress in 20000 steps");
}

/// Parses the DTLS records inside one datagram into (content_type, body)
/// pairs -- enough structure for wire-shape assertions.
pub fn records(datagram: &[u8]) -> Vec<(u8, Vec<u8>)> {
  let mut out = Vec::new();
  let mut rest = datagram;
  while rest.len() >= 13 {
    let ctype = rest[0];
    let len = usize::from(u16::from_be_bytes([rest[11], rest[12]]));
    if rest.len() < 13 + len {
      break;
    }
    out.push((ctype, rest[13..13 + len].to_vec()));
    rest = &rest[13 + len..];
  }
  out
}

/// Every plaintext alert (level, description) found in captured sends.
/// Encrypted alerts (post-CCS epochs) surface too, but their bodies are
/// noise; callers filter by the 2-byte plaintext length.
pub fn plaintext_alerts(sent: &[Vec<u8>]) -> Vec<(u8, u8)> {
  let mut out = Vec::new();
  for dgram in sent {
    for (ctype, body) in records(dgram) {
      if ctype == 21 && body.len() == 2 {
        out.push((body[0], body[1]));
      }
    }
  }
  out
}

pub fn fresh_cid() -> [u8; CID_LEN] {
  let mut cid = [0u8; CID_LEN];
  // Deterministic-ish uniqueness is enough for tests.
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .subsec_nanos();
  cid[..4].copy_from_slice(&nanos.to_be_bytes());
  cid[4..].copy_from_slice(&std::process::id().to_be_bytes());
  cid
}
