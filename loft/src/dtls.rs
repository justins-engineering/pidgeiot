//! CoAP-over-DTLS/UDP listener (coaps, 5684/udp) -- the PRIMARY device
//! transport: PSM'd cellular devices sleep through their NAT bindings, and
//! DTLS+PSK is the cheapest secure wake-and-send available to them.
//!
//! Model: one OS thread owns the listening socket and demultiplexes
//! datagrams by source address into per-connection channels; each
//! connection runs on its own thread with blocking OpenSSL DTLS over a
//! datagram-preserving Read/Write adapter (`DgramIo`). See main.rs for why
//! thread-per-connection.
//!
//! Anti-amplification / pre-auth posture: `SslOptions::COOKIE_EXCHANGE` is
//! enabled with a per-connection random cookie -- OpenSSL answers an
//! initial ClientHello with a small HelloVerifyRequest and does no further
//! handshake processing (in particular, no PSK lookup, which is our only
//! potentially-expensive pre-auth step) until the client echoes the cookie
//! from its claimed source address. A spoofed source therefore costs one
//! small reply plus one parked thread that idles out; it never reaches
//! dovecote. The PSK resolver itself is additionally rate-shaped by its
//! negative cache.
//!
//! Known gap: the safe `openssl` crate doesn't expose
//! `DTLSv1_get_timeout`/`DTLSv1_handle_timeout`, so this server does not
//! proactively retransmit its own handshake flights on a quiet timer.
//! DTLS clients retransmit their flights on timeout and OpenSSL re-sends
//! ours in response, which converges for every real client; an
//! `SSL_ctrl`-based shim (both are macros over it) could drive proper
//! server-side timers from the `WouldBlock` arm of `complete_handshake`.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use openssl::ex_data::Index;
use openssl::ssl::{ErrorCode, HandshakeError, Ssl, SslContext, SslMethod, SslOptions, SslStream};

use crate::coap::message::{Message, code};
use crate::coap::udp::{Datagram, MessageType};
use crate::config::Config;
use crate::handler::{DeviceSession, Handler, Transport};
use crate::psk::PskResolver;
use crate::tls_common::{authenticated_session, build_psk_server_context};
use crate::upstream::Dovecote;

const MAX_CONNECTIONS: usize = 4096;
const CONN_CHANNEL_DEPTH: usize = 32;
const READ_TICK: Duration = Duration::from_secs(1);
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
const IDLE_DEADLINE: Duration = Duration::from_secs(300);
/// Path MTU assumption for handshake flights; CoAP responses stay under it
/// via the handler's spontaneous Block2 (1024-byte blocks).
const DTLS_MTU: u32 = 1400;

/// Per-connection HelloVerifyRequest cookie (random, stashed on the Ssl).
static COOKIE_EX_INDEX: LazyLock<Index<Ssl, [u8; 16]>> =
  LazyLock::new(|| Ssl::new_ex_index().expect("cookie ex index"));

/// Datagram-boundary-preserving blocking IO adapter: reads pop exactly one
/// datagram from the demux channel (WouldBlock on a quiet tick, so the
/// caller can check deadlines), writes send exactly one datagram to the
/// peer. OpenSSL's DTLS stack requires both properties of its BIO.
struct DgramIo {
  rx: Receiver<Vec<u8>>,
  sock: UdpSocket,
  peer: SocketAddr,
}

impl Read for DgramIo {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    match self.rx.recv_timeout(READ_TICK) {
      Ok(dgram) => {
        let n = dgram.len().min(buf.len());
        buf[..n].copy_from_slice(&dgram[..n]);
        Ok(n)
      }
      Err(RecvTimeoutError::Timeout) => Err(io::Error::new(io::ErrorKind::WouldBlock, "tick")),
      Err(RecvTimeoutError::Disconnected) => Ok(0),
    }
  }
}

impl Write for DgramIo {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    self.sock.send_to(buf, self.peer)
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

type ConnMap = Arc<Mutex<HashMap<SocketAddr, SyncSender<Vec<u8>>>>>;

pub fn run(
  config: &Config,
  resolver: Arc<PskResolver>,
  handler: Arc<Handler<Dovecote>>,
  rt: tokio::runtime::Handle,
) {
  if let Err(e) = run_inner(config, resolver, handler, rt) {
    tracing::error!(error = %e, "DTLS listener failed");
  }
}

fn run_inner(
  config: &Config,
  resolver: Arc<PskResolver>,
  handler: Arc<Handler<Dovecote>>,
  rt: tokio::runtime::Handle,
) -> anyhow::Result<()> {
  let ctx = build_context(resolver)?;
  let sock = UdpSocket::bind(&config.udp_listen)?;
  tracing::info!(addr = %config.udp_listen, "DTLS/UDP listener up");

  let conns: ConnMap = Arc::new(Mutex::new(HashMap::new()));
  let mut buf = vec![0u8; 65535];

  loop {
    let (len, peer) = match sock.recv_from(&mut buf) {
      Ok(x) => x,
      Err(e) => {
        tracing::warn!(error = %e, "recv_from failed");
        continue;
      }
    };
    let dgram = buf[..len].to_vec();

    let mut map = conns.lock().expect("conn map lock");
    match map.get(&peer) {
      Some(tx) => match tx.try_send(dgram) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
          // Backpressure: drop the datagram, UDP semantics.
          tracing::debug!(%peer, "connection channel full, dropping datagram");
        }
        Err(TrySendError::Disconnected(dgram)) => {
          map.remove(&peer);
          spawn_connection(&ctx, &sock, peer, dgram, &conns, &handler, &rt, &mut map);
        }
      },
      None => {
        if map.len() >= MAX_CONNECTIONS {
          tracing::warn!(%peer, "connection cap reached, dropping new peer");
          continue;
        }
        spawn_connection(&ctx, &sock, peer, dgram, &conns, &handler, &rt, &mut map);
      }
    }
  }
}

fn build_context(resolver: Arc<PskResolver>) -> anyhow::Result<SslContext> {
  let mut builder = build_psk_server_context(SslMethod::dtls(), true, resolver)?;

  builder.set_options(SslOptions::COOKIE_EXCHANGE);

  builder.set_cookie_generate_cb(|ssl, cookie_out| {
    let mut cookie = [0u8; 16];
    openssl::rand::rand_bytes(&mut cookie)?;
    ssl.set_ex_data(*COOKIE_EX_INDEX, cookie);
    let n = cookie.len().min(cookie_out.len());
    cookie_out[..n].copy_from_slice(&cookie[..n]);
    Ok(n)
  });

  builder.set_cookie_verify_cb(|ssl, cookie| {
    ssl
      .ex_data(*COOKIE_EX_INDEX)
      .is_some_and(|expected| expected.as_slice() == cookie)
  });

  Ok(builder.build())
}

#[allow(clippy::too_many_arguments)]
fn spawn_connection(
  ctx: &SslContext,
  sock: &UdpSocket,
  peer: SocketAddr,
  first_dgram: Vec<u8>,
  conns: &ConnMap,
  handler: &Arc<Handler<Dovecote>>,
  rt: &tokio::runtime::Handle,
  map: &mut HashMap<SocketAddr, SyncSender<Vec<u8>>>,
) {
  let (tx, rx) = std::sync::mpsc::sync_channel(CONN_CHANNEL_DEPTH);
  // The datagram that created this connection is its first input.
  let _ = tx.try_send(first_dgram);
  map.insert(peer, tx);

  let ctx = ctx.clone();
  let conns = conns.clone();
  let handler = handler.clone();
  let rt = rt.clone();
  let sock = match sock.try_clone() {
    Ok(s) => s,
    Err(e) => {
      tracing::error!(error = %e, "socket clone failed");
      map.remove(&peer);
      return;
    }
  };

  let spawned = std::thread::Builder::new()
    .name(format!("dtls-{peer}"))
    .spawn(move || {
      connection_thread(ctx, sock, peer, rx, &handler, &rt);
      conns.lock().expect("conn map lock").remove(&peer);
    });
  if let Err(e) = spawned {
    tracing::error!(error = %e, "connection thread spawn failed");
    map.remove(&peer);
  }
}

fn connection_thread(
  ctx: SslContext,
  sock: UdpSocket,
  peer: SocketAddr,
  rx: Receiver<Vec<u8>>,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
) {
  let io = DgramIo { rx, sock, peer };

  let mut ssl = match Ssl::new(&ctx) {
    Ok(s) => s,
    Err(e) => {
      tracing::error!(error = %e, "Ssl::new failed");
      return;
    }
  };
  if let Err(e) = ssl.set_mtu(DTLS_MTU) {
    tracing::error!(error = %e, "set_mtu failed");
    return;
  }

  let Some(mut stream) = complete_handshake(ssl, io, peer) else {
    return;
  };

  let Some((identity, secret)) = authenticated_session(stream.ssl()) else {
    // Unreachable with PSK-only ciphersuites, but never serve a session
    // whose identity we can't name.
    tracing::error!(%peer, "handshake completed without an authenticated identity");
    return;
  };
  tracing::info!(%peer, identity, "DTLS session established");

  let session = DeviceSession {
    pigeon_id: identity,
    secret,
    peer: peer.to_string(),
  };

  serve_datagrams(&mut stream, &session, handler, rt);
  tracing::debug!(%peer, "DTLS session closed");
}

fn complete_handshake(ssl: Ssl, io: DgramIo, peer: SocketAddr) -> Option<SslStream<DgramIo>> {
  let started = Instant::now();
  let mut result = ssl.accept(io);
  loop {
    match result {
      Ok(stream) => return Some(stream),
      Err(HandshakeError::WouldBlock(mid)) => {
        if started.elapsed() > HANDSHAKE_DEADLINE {
          tracing::debug!(%peer, "handshake deadline exceeded");
          return None;
        }
        // Retransmission integration point: a DTLSv1_handle_timeout shim
        // (SSL_ctrl-based) belongs here, driving server-side flight
        // retransmission on quiet ticks. Until then convergence relies on
        // client-side retransmission (see module docs).
        result = mid.handshake();
      }
      Err(HandshakeError::Failure(mid)) => {
        tracing::info!(%peer, error = %mid.error(), "DTLS handshake failed");
        return None;
      }
      Err(HandshakeError::SetupFailure(e)) => {
        tracing::error!(%peer, error = %e, "DTLS handshake setup failed");
        return None;
      }
    }
  }
}

fn serve_datagrams(
  stream: &mut SslStream<DgramIo>,
  session: &DeviceSession,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
) {
  let mut dedup = DedupCache::new();
  // Message id counter for NON responses (which aren't tied to a request
  // mid the way ACKs are).
  let mut next_mid: u16 = rand_u16();
  let mut last_activity = Instant::now();
  let mut buf = vec![0u8; 65535];

  loop {
    match stream.ssl_read(&mut buf) {
      Ok(0) => return,
      Ok(n) => {
        last_activity = Instant::now();
        if let Some(reply) =
          process_datagram(&buf[..n], session, handler, rt, &mut dedup, &mut next_mid)
          && let Err(e) = stream.ssl_write(&reply)
        {
          tracing::debug!(error = %e, "DTLS write failed");
          return;
        }
      }
      Err(e) if e.code() == ErrorCode::WANT_READ => {
        if last_activity.elapsed() > IDLE_DEADLINE {
          tracing::debug!(peer = %session.peer, "idle timeout");
          return;
        }
      }
      Err(e) if e.code() == ErrorCode::ZERO_RETURN => return,
      Err(e) => {
        tracing::debug!(error = %e, "DTLS read failed");
        return;
      }
    }
  }
}

/// Handles one decrypted datagram; returns the encoded reply datagram, if
/// any. RFC 7252 messaging-layer semantics live here: piggybacked ACKs for
/// CON, NON for NON, RST for an empty CON "ping", and duplicate detection
/// with response replay (a retransmitted CON must get the same ACK back,
/// not a re-executed request).
fn process_datagram(
  bytes: &[u8],
  session: &DeviceSession,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
  dedup: &mut DedupCache,
  next_mid: &mut u16,
) -> Option<Vec<u8>> {
  let dgram = match Datagram::decode(bytes) {
    Ok(d) => d,
    Err(e) => {
      tracing::debug!(error = %e, "undecodable CoAP datagram");
      return None;
    }
  };

  // CoAP ping: empty CON -> RST echoing the message id (RFC 7252 4.3).
  if dgram.message.code == code::EMPTY {
    if dgram.message_type == MessageType::Confirmable {
      return Some(
        Datagram {
          message_type: MessageType::Reset,
          message_id: dgram.message_id,
          message: Message::default(),
        }
        .encode(),
      );
    }
    return None;
  }

  if !code::is_request(dgram.message.code) {
    // A stray response/signaling code over UDP -- ignore.
    return None;
  }

  if let Some(cached) = dedup.get(dgram.message_id) {
    tracing::debug!(
      mid = dgram.message_id,
      "duplicate request, replaying response"
    );
    return Some(cached.to_vec());
  }

  let response = rt.block_on(handler.handle(&dgram.message, session, Transport::Udp));

  let (message_type, message_id) = match dgram.message_type {
    MessageType::Confirmable => (MessageType::Acknowledgement, dgram.message_id),
    _ => {
      *next_mid = next_mid.wrapping_add(1);
      (MessageType::NonConfirmable, *next_mid)
    }
  };

  let encoded = Datagram {
    message_type,
    message_id,
    message: response,
  }
  .encode();

  dedup.insert(dgram.message_id, encoded.clone());
  Some(encoded)
}

fn rand_u16() -> u16 {
  let mut b = [0u8; 2];
  let _ = openssl::rand::rand_bytes(&mut b);
  u16::from_be_bytes(b)
}

/// Duplicate-detection cache, per connection: message id -> the encoded
/// response already sent. RFC 7252's EXCHANGE_LIFETIME is ~247s; entries
/// live a bounded 150s (a client still retransmitting a mid after that has
/// long since given up per default transmission parameters), capped to
/// keep a hostile peer from ballooning memory.
struct DedupCache {
  entries: HashMap<u16, (Vec<u8>, Instant)>,
  ttl: Duration,
  cap: usize,
}

impl DedupCache {
  fn new() -> DedupCache {
    DedupCache {
      entries: HashMap::new(),
      ttl: Duration::from_secs(150),
      cap: 256,
    }
  }

  fn get(&mut self, mid: u16) -> Option<&[u8]> {
    let now = Instant::now();
    let ttl = self.ttl;
    self
      .entries
      .retain(|_, (_, at)| now.duration_since(*at) < ttl);
    self.entries.get(&mid).map(|(bytes, _)| bytes.as_slice())
  }

  fn insert(&mut self, mid: u16, response: Vec<u8>) {
    if self.entries.len() >= self.cap {
      // Evict the oldest entry rather than refusing to record.
      if let Some(oldest) = self
        .entries
        .iter()
        .min_by_key(|(_, (_, at))| *at)
        .map(|(k, _)| *k)
      {
        self.entries.remove(&oldest);
      }
    }
    self.entries.insert(mid, (response, Instant::now()));
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn dedup_replays_within_ttl() {
    let mut cache = DedupCache::new();
    cache.insert(7, vec![1, 2, 3]);
    assert_eq!(cache.get(7), Some([1, 2, 3].as_slice()));
    assert_eq!(cache.get(8), None);
  }

  #[test]
  fn dedup_expires() {
    let mut cache = DedupCache {
      entries: HashMap::new(),
      ttl: Duration::ZERO,
      cap: 256,
    };
    cache.insert(7, vec![1]);
    assert_eq!(cache.get(7), None);
  }

  #[test]
  fn dedup_evicts_oldest_at_cap() {
    let mut cache = DedupCache {
      entries: HashMap::new(),
      ttl: Duration::from_secs(150),
      cap: 2,
    };
    cache.insert(1, vec![1]);
    std::thread::sleep(Duration::from_millis(5));
    cache.insert(2, vec![2]);
    std::thread::sleep(Duration::from_millis(5));
    cache.insert(3, vec![3]);
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.get(1), None, "oldest entry evicted");
    assert!(cache.get(2).is_some());
    assert!(cache.get(3).is_some());
  }
}
