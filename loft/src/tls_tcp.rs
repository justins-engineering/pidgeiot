//! CoAP-over-TLS/TCP listener (coaps+tcp, 5684/tcp, RFC 8323) -- the
//! secondary transport, for `~/pigeon` builds that pick TCP over the
//! primary DTLS/UDP. Same PSK story and thread-per-connection model as
//! the DTLS listener.
//!
//! RFC 8323 messaging notes:
//! - We send our CSM (7.01) immediately after the handshake, per section
//!   5.3. We tolerate peers that never send theirs -- the minimal
//!   `~/pigeon` client sends requests directly, and rejecting it would be
//!   standards-compliant but pointless.
//! - Ping (7.02) gets Pong (7.03) with the same token; Release/Abort end
//!   the connection.
//! - No message ids, no ACKs, no retransmission -- TCP owns reliability.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use openssl::ssl::{HandshakeError, Ssl, SslContext, SslMethod, SslStream};

use crate::coap::message::{Message, code};
use crate::coap::tcp::{FrameDecoder, encode_frame};
use crate::config::Config;
use crate::handler::{DeviceSession, Handler, Transport};
use crate::psk::PskResolver;
use crate::quota::{ConnQuota, MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP};
use crate::tls_common::{authenticated_session, build_psk_server_context};
use crate::upstream::Dovecote;

/// One blocking read tick; idle handling rides on TCP read timeouts.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Wall-clock bound on the WHOLE handshake, same figure as the DTLS
/// listener's. A per-recv timeout alone is not a bound: each byte the peer
/// dribbles in restarts it, so a slow-trickle client could sit inside
/// accept() forever holding a pre-auth connection slot. Enforced by
/// `GuardedTcp` on every read and write, plus the accept loop's own check
/// on quiet ticks.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
/// Socket-timeout tick during the handshake -- how often a *silent* peer
/// surfaces WouldBlock out of accept() so the deadline can be checked.
const HANDSHAKE_TICK: Duration = Duration::from_secs(1);

/// `TcpStream` wrapper that fails handshake IO outright once the wall-clock
/// deadline passes. The tick timeouts on the socket only bound silence:
/// while bytes keep arriving, OpenSSL calls straight back into recv without
/// ever surfacing WouldBlock, so a peer pacing one valid byte per tick
/// would otherwise stay inside a single accept() call for as long as it
/// can stretch a handshake message. Checking the deadline on every read
/// and write bounds the handshake regardless of pacing. Disarmed once the
/// session is established; from then on IO is a plain passthrough.
#[derive(Debug)]
struct GuardedTcp {
  tcp: TcpStream,
  deadline: Instant,
  armed: bool,
}

impl GuardedTcp {
  fn expired(&self) -> Option<io::Error> {
    (self.armed && Instant::now() >= self.deadline)
      .then(|| io::Error::new(io::ErrorKind::TimedOut, "handshake deadline exceeded"))
  }
}

impl Read for GuardedTcp {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    if let Some(e) = self.expired() {
      return Err(e);
    }
    self.tcp.read(buf)
  }
}

impl Write for GuardedTcp {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    if let Some(e) = self.expired() {
      return Err(e);
    }
    self.tcp.write(buf)
  }

  fn flush(&mut self) -> io::Result<()> {
    self.tcp.flush()
  }
}

pub fn run(
  config: &Config,
  resolver: Arc<PskResolver>,
  handler: Arc<Handler<Dovecote>>,
  rt: tokio::runtime::Handle,
) {
  if let Err(e) = run_inner(config, resolver, handler, rt) {
    tracing::error!(error = %e, "TLS/TCP listener failed");
  }
}

fn run_inner(
  config: &Config,
  resolver: Arc<PskResolver>,
  handler: Arc<Handler<Dovecote>>,
  rt: tokio::runtime::Handle,
) -> anyhow::Result<()> {
  let ctx = build_psk_server_context(SslMethod::tls_server(), false, resolver)?.build();
  let listener = TcpListener::bind(&config.tcp_listen)?;
  tracing::info!(addr = %config.tcp_listen, "TLS/TCP listener up");

  let quota = ConnQuota::new(MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP);

  for stream in listener.incoming() {
    let stream = match stream {
      Ok(s) => s,
      Err(e) => {
        tracing::warn!(error = %e, "accept failed");
        continue;
      }
    };

    // Admission is keyed on the source IP; a socket that can't name its
    // peer can't be accounted, so it isn't admitted.
    let Ok(peer_ip) = stream.peer_addr().map(|a| a.ip()) else {
      continue;
    };
    let Some(permit) = quota.try_acquire(peer_ip) else {
      tracing::warn!(ip = %peer_ip, "connection quota reached, refusing TCP peer");
      continue;
    };

    let ctx = ctx.clone();
    let handler = handler.clone();
    let rt = rt.clone();
    let spawned = std::thread::Builder::new()
      .name("coaps-tcp-conn".into())
      .spawn(move || {
        // The permit rides the connection thread so every exit path --
        // handshake failure, deadline abort, session close -- releases its
        // quota slots; a failed spawn drops the closure and settles the
        // same way.
        let _permit = permit;
        connection_thread(&ctx, stream, &handler, &rt);
      });
    if let Err(e) = spawned {
      tracing::error!(error = %e, "connection thread spawn failed");
    }
  }
  Ok(())
}

fn connection_thread(
  ctx: &SslContext,
  tcp: TcpStream,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
) {
  let peer = match tcp.peer_addr() {
    Ok(a) => a.to_string(),
    Err(_) => "unknown".to_string(),
  };
  // Tick-length timeouts on both directions while the handshake runs, so a
  // peer that goes quiet (or never drains our flight) surfaces WouldBlock
  // once per tick for the deadline check below; GuardedTcp covers the
  // complementary case of a peer that keeps bytes flowing.
  if tcp.set_read_timeout(Some(HANDSHAKE_TICK)).is_err()
    || tcp.set_write_timeout(Some(HANDSHAKE_TICK)).is_err()
    || tcp.set_nodelay(true).is_err()
  {
    return;
  }

  let ssl = match Ssl::new(ctx) {
    Ok(s) => s,
    Err(e) => {
      tracing::error!(error = %e, "Ssl::new failed");
      return;
    }
  };

  let started = Instant::now();
  let mut attempt = ssl.accept(GuardedTcp {
    tcp,
    deadline: started + HANDSHAKE_DEADLINE,
    armed: true,
  });
  let mut stream = loop {
    match attempt {
      Ok(s) => break s,
      Err(HandshakeError::WouldBlock(mid)) => {
        if started.elapsed() > HANDSHAKE_DEADLINE {
          tracing::info!(%peer, "TLS handshake deadline exceeded");
          return;
        }
        attempt = mid.handshake();
      }
      Err(HandshakeError::Failure(mid)) => {
        tracing::info!(%peer, error = %mid.error(), "TLS handshake failed");
        return;
      }
      Err(e) => {
        tracing::info!(%peer, error = %e, "TLS handshake failed");
        return;
      }
    }
  };

  let Some((identity, token)) = authenticated_session(stream.ssl()) else {
    tracing::error!(%peer, "handshake completed without an authenticated identity");
    return;
  };
  tracing::info!(%peer, identity, "coaps+tcp session established");

  stream.get_mut().armed = false;
  let _ = stream.get_ref().tcp.set_read_timeout(Some(IDLE_TIMEOUT));
  // Established sessions write firmware blocks to devices that can stall
  // far longer than a handshake tick; let the OS pace those writes again.
  let _ = stream.get_ref().tcp.set_write_timeout(None);

  let session = DeviceSession {
    pigeon_id: identity,
    token,
    peer,
  };

  // Our CSM: default settings are fine for this surface (no
  // Max-Message-Size or Block-Wise-Transfer options -- RFC 8323 defaults
  // apply: 1152-byte default max message size hint, which our Block2
  // firmware path respects anyway).
  let csm = Message {
    code: code::CSM,
    ..Default::default()
  };
  if stream.write_all(&encode_frame(&csm)).is_err() {
    return;
  }

  serve_frames(&mut stream, &session, handler, rt);
  tracing::debug!(peer = %session.peer, "coaps+tcp session closed");
}

fn serve_frames(
  stream: &mut SslStream<GuardedTcp>,
  session: &DeviceSession,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
) {
  let mut decoder = FrameDecoder::default();
  let mut buf = vec![0u8; 16 * 1024];

  loop {
    let n = match stream.read(&mut buf) {
      Ok(0) => return,
      Ok(n) => n,
      Err(e) => {
        tracing::debug!(peer = %session.peer, error = %e, "read ended");
        return;
      }
    };
    decoder.extend(&buf[..n]);

    loop {
      let msg = match decoder.next_frame() {
        Ok(Some(m)) => m,
        Ok(None) => break,
        Err(e) => {
          tracing::debug!(peer = %session.peer, error = %e, "bad frame, aborting connection");
          let abort = Message {
            code: code::ABORT,
            ..Default::default()
          };
          let _ = stream.write_all(&encode_frame(&abort));
          return;
        }
      };

      let reply = match msg.code {
        code::CSM => None, // peer's settings; defaults are acceptable
        code::PING => Some(Message {
          code: code::PONG,
          token: msg.token.clone(),
          ..Default::default()
        }),
        code::PONG => None,
        code::RELEASE | code::ABORT => return,
        c if code::is_request(c) => {
          Some(rt.block_on(handler.handle(&msg, session, Transport::Tcp)))
        }
        _ => None, // stray response code; ignore
      };

      if let Some(reply) = reply
        && stream.write_all(&encode_frame(&reply)).is_err()
      {
        return;
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Data availability must not bypass the deadline: the guard refuses IO
  /// even with a byte already queued, and reverts to a passthrough once
  /// disarmed.
  #[test]
  fn guarded_tcp_refuses_io_past_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(addr).unwrap();
    let (server, _) = listener.accept().unwrap();
    client.write_all(b"x").unwrap();

    let mut guarded = GuardedTcp {
      tcp: server,
      deadline: Instant::now(),
      armed: true,
    };
    let mut buf = [0u8; 1];
    assert_eq!(
      guarded.read(&mut buf).unwrap_err().kind(),
      io::ErrorKind::TimedOut
    );
    assert_eq!(
      guarded.write(b"y").unwrap_err().kind(),
      io::ErrorKind::TimedOut
    );

    guarded.armed = false;
    assert_eq!(guarded.read(&mut buf).unwrap(), 1);
    assert_eq!(&buf, b"x");
  }
}
