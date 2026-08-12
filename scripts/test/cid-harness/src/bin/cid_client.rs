//! A DTLS-PSK CoAP-ping client for the netns rebind harness, built on the
//! same `mbedtls-ffi-shim` the server terminates with, so both ends of the
//! RFC 9146 CID extension are the fielded implementation. It handshakes
//! once, then sends CoAP pings (empty CON) and expects RSTs, printing one
//! machine-parsable line per exchange plus a RESULT summary. When a ping
//! goes unanswered it rebuilds the DTLS session over the same socket and
//! counts the re-handshake -- the difference the whole harness turns on:
//! across a NAT rebind a CID client keeps its one handshake, a no-CID
//! client is forced into a second.

use std::io::Write;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mbedtls_ffi_shim::{
  Config, HandshakeStatus, MbedIo, ReadStatus, RecvOutcome, SendOutcome, Session, TimerState,
};

const IDENTITY: &[u8] = b"cid-harness-pigeon";
const PSK: &[u8] = b"0123456789abcdef0123456789abcdef";
/// One blocking read never parks longer than this, so the DTLS
/// retransmission timer is polled promptly during a handshake.
const READ_SLICE: Duration = Duration::from_millis(20);

struct SockIo {
  sock: UdpSocket,
}

impl MbedIo for SockIo {
  fn send(&mut self, buf: &[u8]) -> SendOutcome {
    match self.sock.send(buf) {
      Ok(n) => SendOutcome::Sent(n),
      Err(_) => SendOutcome::Failed,
    }
  }

  fn recv(&mut self, buf: &mut [u8], timer: &TimerState) -> RecvOutcome {
    let slice = timer
      .remaining()
      .map(|r| r.min(READ_SLICE))
      .unwrap_or(READ_SLICE);
    let _ = self
      .sock
      .set_read_timeout(Some(slice.max(Duration::from_millis(1))));
    match self.sock.recv(buf) {
      Ok(n) => RecvOutcome::Data(n),
      Err(e)
        if e.kind() == std::io::ErrorKind::WouldBlock
          || e.kind() == std::io::ErrorKind::TimedOut =>
      {
        if timer.final_expired() {
          RecvOutcome::TimerExpired
        } else {
          RecvOutcome::WantRead
        }
      }
      Err(_) => RecvOutcome::Failed,
    }
  }
}

/// A fresh DTLS session over the given (connected) socket. Returns None if
/// the handshake cannot complete within `patience`.
fn handshake(
  config: &Arc<Config>,
  sock: &UdpSocket,
  offer_cid: bool,
  patience: Duration,
) -> Option<Session<SockIo>> {
  let io = SockIo {
    sock: sock.try_clone().expect("clone socket"),
  };
  let mut session = Session::new(config, io).expect("client session");
  if offer_cid {
    session.offer_zero_length_cid().expect("cid offer");
  }
  let deadline = Instant::now() + patience;
  loop {
    match session.handshake() {
      HandshakeStatus::Done => return Some(session),
      HandshakeStatus::WantRead | HandshakeStatus::WantWrite => {
        if Instant::now() >= deadline {
          return None;
        }
      }
      HandshakeStatus::HelloVerifyRequired => {} // client never sees this as terminal
      HandshakeStatus::Failed(e) => {
        eprintln!("handshake failed: {e}");
        return None;
      }
    }
  }
}

/// One CoAP ping exchange: empty CON out, RST with the echoed message id
/// back. Returns true on the matching RST within `patience`.
fn ping(session: &mut Session<SockIo>, mid: u16, patience: Duration) -> bool {
  let [hi, lo] = mid.to_be_bytes();
  if session.write(&[0x40, 0x00, hi, lo]).is_err() {
    return false;
  }
  let deadline = Instant::now() + patience;
  let mut buf = [0u8; 64];
  loop {
    match session.read(&mut buf) {
      ReadStatus::Data(n) => return n == 4 && buf[..4] == [0x70, 0x00, hi, lo],
      ReadStatus::WantRead | ReadStatus::WantWrite => {
        if Instant::now() >= deadline {
          return false;
        }
      }
      ReadStatus::PeerClosed | ReadStatus::Failed(_) => return false,
    }
  }
}

/// Print and flush: stdout is block-buffered when redirected to a file,
/// and the harness polls this output line-by-line to time the rebind
/// event, so every progress line must hit the pipe immediately.
fn emit(line: &str) {
  println!("{line}");
  let _ = std::io::stdout().flush();
}

fn arg(name: &str, default: &str) -> String {
  let args: Vec<String> = std::env::args().collect();
  args
    .windows(2)
    .find(|w| w[0] == name)
    .map(|w| w[1].clone())
    .unwrap_or_else(|| default.to_string())
}

fn main() {
  let target = arg("--target", "127.0.0.1:5684");
  let offer_cid = arg("--mode", "cid") == "cid";
  let exchanges: usize = arg("--exchanges", "6").parse().expect("exchanges");
  let interval = Duration::from_millis(arg("--interval-ms", "500").parse().expect("interval"));

  let sock = UdpSocket::bind("0.0.0.0:0").expect("bind client");
  sock.connect(&target).expect("connect");

  let config = Arc::new(Config::client(IDENTITY, PSK, offer_cid).expect("client config"));
  let patience = Duration::from_secs(8);

  let mut handshakes = 0usize;
  let Some(mut session) = handshake(&config, &sock, offer_cid, patience) else {
    emit(&format!(
      "RESULT handshakes=0 exchanges_ok=0/{exchanges} FAIL_HANDSHAKE"
    ));
    std::process::exit(1);
  };
  handshakes += 1;
  let cid = session.peer_cid().expect("peer cid");
  emit(&format!(
    "HANDSHAKE #{handshakes} suite={} cid_negotiated={}",
    session.ciphersuite().unwrap_or_default(),
    cid.negotiated
  ));

  let mut ok = 0usize;
  let mut mid: u16 = 0x1000;
  for i in 0..exchanges {
    mid = mid.wrapping_add(1);
    if ping(&mut session, mid, Duration::from_secs(2)) {
      ok += 1;
      emit(&format!("EXCHANGE {i} ok"));
    } else {
      // Unanswered: recover with a fresh handshake over the same socket,
      // the way a real client would after losing its session. A CID
      // client should never reach here across a rebind; a no-CID client
      // must, and the re-handshake count is what the harness asserts on.
      emit(&format!("EXCHANGE {i} timeout, re-handshaking"));
      match handshake(&config, &sock, offer_cid, patience) {
        Some(s) => {
          session = s;
          handshakes += 1;
          emit(&format!("HANDSHAKE #{handshakes} (recovery)"));
          if ping(&mut session, mid, Duration::from_secs(2)) {
            ok += 1;
            emit(&format!("EXCHANGE {i} ok (after re-handshake)"));
          }
        }
        None => emit(&format!("EXCHANGE {i} unrecoverable")),
      }
    }
    std::thread::sleep(interval);
  }

  emit(&format!(
    "RESULT handshakes={handshakes} exchanges_ok={ok}/{exchanges}"
  ));
}
