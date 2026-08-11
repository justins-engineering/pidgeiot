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
//! Anti-amplification / pre-auth posture: every datagram from an unknown
//! source is answered on the listener thread itself through OpenSSL's
//! stateless `DTLSv1_listen` (via the `dtls-ffi-shim` workspace crate),
//! over a single reusable pending stream. The HelloVerifyRequest cookie is
//! an HMAC of the claimed source address under a process-lifetime random
//! key, so generating and checking it needs no per-source state at all:
//! until a source has echoed a valid cookie -- proving it can receive at
//! the address it claims -- it owns no conn-map entry, no channel, no
//! socket handle, and no thread here. A spoofed or silent source costs one
//! reply smaller than the ClientHello that provoked it (so no
//! amplification either) and nothing held. Only a verified cookie promotes
//! the pending stream into a real connection with its own thread; the PSK
//! lookup (our only potentially-expensive pre-auth step) happens later
//! still, inside that connection's handshake, and is additionally
//! rate-shaped by the resolver's negative cache.
//!
//! `DTLSv1_listen(3)` requires callers to connect the socket to the
//! verified peer afterwards so replies cannot be redirected; this demux
//! provides the equivalent -- a promoted stream's writes are pinned to the
//! verified source address, and its reads come only from a channel the
//! listener fills strictly by exact source-address match.
//!
//! Handshake retransmission: OpenSSL checks its own DTLS retransmission
//! timer on every re-entry into a pending handshake and re-sends the last
//! flight itself once the timer has elapsed -- `complete_handshake`'s
//! 1s-tick `WouldBlock` loop provides exactly that re-entry cadence, so
//! lost server flights are retransmitted without driving
//! `DTLSv1_handle_timeout` by hand (DTLS's initial RTO is 1s, so the tick
//! adds no meaningful latency). A poll-driven single-loop listener would
//! lose that free re-entry and need the shim's `SSL_ctrl`-based timeout
//! wrappers on top; today only `DTLSv1_listen` itself is driven through
//! `dtls-ffi-shim`.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use dtls_ffi_shim::dtls_ffi::{self, ListenOutcome};
use openssl::error::ErrorStack;
use openssl::ex_data::Index;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::sign::Signer;
use openssl::ssl::{ErrorCode, Ssl, SslContext, SslMethod, SslOptions, SslStream};

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

/// The claimed source address of the datagram currently being fed to an
/// `Ssl`, stashed where both cookie callbacks can reach it. The cookie is
/// an HMAC over exactly this address, and the value must ride along on the
/// `Ssl` when a verified stream is handed to its connection thread: with
/// `SslOptions::COOKIE_EXCHANGE` set, the post-listen `SSL_accept`
/// re-verifies the buffered ClientHello's cookie there.
static PEER_EX_INDEX: LazyLock<Index<Ssl, SocketAddr>> =
  LazyLock::new(|| Ssl::new_ex_index().expect("peer ex index"));

/// Datagram-boundary-preserving blocking IO adapter: reads pop exactly one
/// datagram from the demux channel (WouldBlock on a quiet tick, so the
/// caller can check deadlines), writes send exactly one datagram to the
/// peer. OpenSSL's DTLS stack requires both properties of its BIO.
struct DgramIo {
  rx: Receiver<Vec<u8>>,
  sock: UdpSocket,
  peer: SocketAddr,
  /// How long a read may park waiting for a datagram before surfacing
  /// WouldBlock: zero while the stream is the listener thread's shared
  /// pending-listen stream (the listener must never sleep on one source's
  /// silence), one READ_TICK once a connection thread owns it.
  tick: Duration,
}

impl Read for DgramIo {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    match self.rx.recv_timeout(self.tick) {
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
  listen_loop(sock, &ctx, &conns, &handler, &rt)
}

/// The demux loop, taking its socket and conn map from the caller so tests
/// can drive it on an ephemeral port and observe exactly what state each
/// source address has (or has not) earned.
fn listen_loop(
  sock: UdpSocket,
  ctx: &SslContext,
  conns: &ConnMap,
  handler: &Arc<Handler<Dovecote>>,
  rt: &tokio::runtime::Handle,
) -> anyhow::Result<()> {
  // The single not-yet-verified stream every unknown source is funneled
  // through; rebuilt lazily after a promotion or a fatal listen error.
  let mut pending: Option<PendingListen> = None;
  let mut buf = vec![0u8; 65535];

  loop {
    let (len, peer) = match sock.recv_from(&mut buf) {
      Ok(x) => x,
      Err(e) => {
        tracing::warn!(error = %e, "recv_from failed");
        continue;
      }
    };
    let mut dgram = buf[..len].to_vec();

    {
      let mut map = conns.lock().expect("conn map lock");
      if let Some(tx) = map.get(&peer) {
        match tx.try_send(dgram) {
          Ok(()) => continue,
          Err(TrySendError::Full(_)) => {
            // Backpressure: drop the datagram, UDP semantics.
            tracing::debug!(%peer, "connection channel full, dropping datagram");
            continue;
          }
          Err(TrySendError::Disconnected(d)) => {
            // The connection thread is gone, so this source is unknown
            // again and must re-earn its slot through the cookie exchange
            // like anyone else.
            map.remove(&peer);
            dgram = d;
          }
        }
      }
      if map.len() >= MAX_CONNECTIONS {
        tracing::warn!(%peer, "connection cap reached, dropping new peer");
        continue;
      }
    }

    // Unknown source: drive the stateless cookie exchange right here.
    // Nothing per-source is allocated on this path -- the pending stream,
    // its channel, and its cloned socket handle are shared across every
    // unverified source, and are only ever promoted, never multiplied.
    if pending.is_none() {
      match PendingListen::new(ctx, &sock) {
        Ok(pl) => pending = Some(pl),
        Err(e) => {
          tracing::error!(error = %e, "pending listen stream setup failed");
          continue;
        }
      }
    }
    let pl = pending.as_mut().expect("pending listen stream present");

    // Both the HelloVerifyRequest reply and the cookie HMAC key off the
    // claimed source address; stamp it before every listen call, since the
    // previous datagram may have come from someone else.
    pl.stream.get_mut().peer = peer;
    dtls_ffi::ssl_mut(&mut pl.stream).set_ex_data(*PEER_EX_INDEX, peer);
    if pl.tx.try_send(dgram).is_err() {
      // dtlsv1_listen drains the channel on every call, so a full or
      // closed channel means the pending stream is wedged; rebuild it.
      pending = None;
      continue;
    }

    match dtls_ffi::dtlsv1_listen(dtls_ffi::ssl_mut(&mut pl.stream)) {
      // Garbage (dropped internally), or a cookie-less ClientHello that
      // was answered with a HelloVerifyRequest straight off this thread.
      // Either way: nothing allocated, nothing held.
      Ok(ListenOutcome::Retry) => {}
      // The outcome's own peer field stays None through rust-openssl's
      // Read+Write bridge (see the shim docs); the demux already knows
      // the address from recv_from.
      Ok(ListenOutcome::Accepted { .. }) => {
        let pl = pending.take().expect("pending listen stream present");
        promote_connection(pl, peer, conns, handler, rt);
      }
      Err(e) => {
        tracing::debug!(%peer, error = %e, "DTLSv1_listen failed");
        pending = None;
      }
    }
  }
}

fn build_context(resolver: Arc<PskResolver>) -> anyhow::Result<SslContext> {
  let mut builder = build_psk_server_context(SslMethod::dtls(), true, resolver)?;

  builder.set_options(SslOptions::COOKIE_EXCHANGE);

  // Process-lifetime HMAC key. A restart only invalidates cookies from
  // exchanges already in flight, which clients recover from by
  // retransmitting their ClientHello and receiving a fresh
  // HelloVerifyRequest.
  let mut key_bytes = [0u8; 32];
  openssl::rand::rand_bytes(&mut key_bytes)?;
  let cookie_key = Arc::new(PKey::hmac(&key_bytes)?);

  let generate_key = Arc::clone(&cookie_key);
  builder.set_cookie_generate_cb(move |ssl, cookie_out| {
    let Some(peer) = ssl.ex_data(*PEER_EX_INDEX).copied() else {
      // The listener stamps the address before every listen call; minting
      // a cookie without one would sever the address binding the cookie
      // exists to prove, so fail the exchange instead.
      return Err(ErrorStack::get());
    };
    let cookie = generate_cookie(&generate_key, &peer)?;
    if cookie.len() > cookie_out.len() {
      return Err(ErrorStack::get());
    }
    cookie_out[..cookie.len()].copy_from_slice(&cookie);
    Ok(cookie.len())
  });

  builder.set_cookie_verify_cb(move |ssl, cookie| {
    ssl
      .ex_data(*PEER_EX_INDEX)
      .copied()
      .is_some_and(|peer| verify_cookie(&cookie_key, &peer, cookie))
  });

  Ok(builder.build())
}

/// Byte encoding of a claimed source address for the cookie HMAC. The two
/// families can never collide: their lengths differ (4+2 vs 16+2).
fn cookie_material(peer: &SocketAddr) -> Vec<u8> {
  let mut material = Vec::with_capacity(18);
  match peer.ip() {
    IpAddr::V4(ip) => material.extend_from_slice(&ip.octets()),
    IpAddr::V6(ip) => material.extend_from_slice(&ip.octets()),
  }
  material.extend_from_slice(&peer.port().to_be_bytes());
  material
}

fn generate_cookie(key: &PKey<Private>, peer: &SocketAddr) -> Result<Vec<u8>, ErrorStack> {
  let mut signer = Signer::new(MessageDigest::sha256(), key)?;
  signer.sign_oneshot_to_vec(&cookie_material(peer))
}

/// Constant-time comparison: the cookie is the only thing standing between
/// a spoofed source and a connection slot, so the check must not leak how
/// many prefix bytes matched.
fn verify_cookie(key: &PKey<Private>, peer: &SocketAddr, cookie: &[u8]) -> bool {
  match generate_cookie(key, peer) {
    Ok(expected) => cookie.len() == expected.len() && openssl::memcmp::eq(cookie, &expected),
    Err(_) => false,
  }
}

/// The listener thread's single not-yet-verified stream. Every datagram
/// from an unknown source is fed through it for the stateless cookie
/// exchange; a verified cookie promotes the whole thing -- stream, channel,
/// socket handle -- into the connection it already is, and a fresh one is
/// built lazily for the next stranger. At most one exists at a time, which
/// is the point: an unverified source can never cause allocation beyond it.
struct PendingListen {
  stream: SslStream<DgramIo>,
  tx: SyncSender<Vec<u8>>,
}

impl PendingListen {
  fn new(ctx: &SslContext, sock: &UdpSocket) -> anyhow::Result<PendingListen> {
    let (tx, rx) = std::sync::mpsc::sync_channel(CONN_CHANNEL_DEPTH);
    let mut ssl = Ssl::new(ctx)?;
    ssl.set_accept_state();
    let io = DgramIo {
      rx,
      sock: sock.try_clone()?,
      // Overwritten before any datagram is fed; never sent to as-is.
      peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
      tick: Duration::ZERO,
    };
    Ok(PendingListen {
      stream: SslStream::new(ssl, io)?,
      tx,
    })
  }
}

/// Turns a cookie-verified pending stream into a real connection: only now
/// does the source own a conn-map slot and an OS thread.
fn promote_connection(
  pl: PendingListen,
  peer: SocketAddr,
  conns: &ConnMap,
  handler: &Arc<Handler<Dovecote>>,
  rt: &tokio::runtime::Handle,
) {
  let PendingListen { stream, tx } = pl;
  let mut map = conns.lock().expect("conn map lock");
  map.insert(peer, tx);

  let conns_for_thread = conns.clone();
  let handler = handler.clone();
  let rt = rt.clone();
  let spawned = std::thread::Builder::new()
    .name(format!("dtls-{peer}"))
    .spawn(move || {
      connection_thread(stream, peer, &handler, &rt);
      conns_for_thread
        .lock()
        .expect("conn map lock")
        .remove(&peer);
    });
  if let Err(e) = spawned {
    tracing::error!(error = %e, "connection thread spawn failed");
    map.remove(&peer);
  }
}

fn connection_thread(
  mut stream: SslStream<DgramIo>,
  peer: SocketAddr,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
) {
  // Reads may park a tick at a time from here on: unlike the listener
  // thread, this thread has nothing else to service, and the tick doubles
  // as the deadline-check cadence for the loops below.
  stream.get_mut().tick = READ_TICK;

  // The MTU must be applied after DTLSv1_listen, whose internal SSL_clear
  // resets DTLS transfer state; the handshake flights SSL_accept is about
  // to send are the first thing that fragments against it anyway.
  if let Err(e) = dtls_ffi::ssl_mut(&mut stream).set_mtu(DTLS_MTU) {
    tracing::error!(error = %e, "set_mtu failed");
    return;
  }

  if !complete_handshake(&mut stream, peer) {
    return;
  }

  let Some((identity, token)) = authenticated_session(stream.ssl()) else {
    // Unreachable with PSK-only ciphersuites, but never serve a session
    // whose identity we can't name.
    tracing::error!(%peer, "handshake completed without an authenticated identity");
    return;
  };
  tracing::info!(%peer, identity, "DTLS session established");

  let session = DeviceSession {
    pigeon_id: identity,
    token,
    peer: peer.to_string(),
  };

  serve_datagrams(&mut stream, &session, handler, rt);
  tracing::debug!(%peer, "DTLS session closed");
}

/// Continues the handshake `DTLSv1_listen` began, bounded by a wall-clock
/// deadline. The cookie exchange already happened statelessly on the
/// listener thread; only the post-cookie flights are driven here.
fn complete_handshake(stream: &mut SslStream<DgramIo>, peer: SocketAddr) -> bool {
  let started = Instant::now();
  loop {
    match stream.accept() {
      Ok(()) => return true,
      Err(e) if matches!(e.code(), ErrorCode::WANT_READ | ErrorCode::WANT_WRITE) => {
        if started.elapsed() > HANDSHAKE_DEADLINE {
          tracing::debug!(%peer, "handshake deadline exceeded");
          return false;
        }
        // Quiet tick: OpenSSL re-checks its retransmission timer on the
        // next accept() re-entry (see module docs), so lost server
        // flights are re-sent without driving DTLSv1_handle_timeout by
        // hand.
      }
      Err(e) => {
        tracing::info!(%peer, error = %e, "DTLS handshake failed");
        return false;
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
  use crate::psk::PskEntry;

  const TEST_IDENTITY: &str = "test-pigeon";
  const TEST_PSK: &str = "0123456789abcdef0123456789abcdef";

  /// Real listener on an ephemeral loopback port, with the conn map
  /// exposed so tests can assert which sources have earned state.
  fn start_listener(rt: &tokio::runtime::Runtime) -> (SocketAddr, ConnMap) {
    let resolver = Arc::new(PskResolver::new(
      Box::new(|identity: &str| {
        Ok((identity == TEST_IDENTITY).then(|| PskEntry {
          psk: TEST_PSK.to_string(),
          token: "test-token".to_string(),
        }))
      }),
      Duration::from_secs(60),
    ));
    let ctx = build_context(resolver).expect("server context");
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind listener");
    let addr = sock.local_addr().expect("listener addr");
    let conns: ConnMap = Arc::new(Mutex::new(HashMap::new()));
    let handler = Arc::new(Handler::new(
      Dovecote::new("http://127.0.0.1:9").expect("upstream stub"),
    ));

    let loop_conns = conns.clone();
    let handle = rt.handle().clone();
    std::thread::spawn(move || {
      let _ = listen_loop(sock, &ctx, &loop_conns, &handler, &handle);
    });
    (addr, conns)
  }

  /// A connected loopback UDP socket as the blocking `Read + Write` shape
  /// rust-openssl expects; the read timeout keeps handshake loops from
  /// hanging a failed test forever.
  struct ClientIo(UdpSocket);

  impl Read for ClientIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      self.0.recv(buf)
    }
  }

  impl Write for ClientIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      self.0.send(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
      Ok(())
    }
  }

  fn connect_client(server: SocketAddr) -> SslStream<ClientIo> {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind client");
    sock.connect(server).expect("connect client");
    sock
      .set_read_timeout(Some(Duration::from_millis(200)))
      .expect("read timeout");

    let mut builder = SslContext::builder(SslMethod::dtls()).expect("client ctx");
    builder
      .set_cipher_list("PSK-AES128-GCM-SHA256")
      .expect("cipher list");
    builder.set_psk_client_callback(|_ssl, _hint, identity_out, psk_out| {
      identity_out[..TEST_IDENTITY.len()].copy_from_slice(TEST_IDENTITY.as_bytes());
      identity_out[TEST_IDENTITY.len()] = 0;
      psk_out[..TEST_PSK.len()].copy_from_slice(TEST_PSK.as_bytes());
      Ok(TEST_PSK.len())
    });
    let ctx = builder.build();

    let mut ssl = Ssl::new(&ctx).expect("client ssl");
    ssl.set_connect_state();
    let mut stream = SslStream::new(ssl, ClientIo(sock)).expect("client stream");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
      match stream.connect() {
        Ok(()) => return stream,
        Err(e) if matches!(e.code(), ErrorCode::WANT_READ | ErrorCode::WANT_WRITE) => {
          assert!(Instant::now() < deadline, "client handshake timed out");
        }
        Err(e) => panic!("client handshake failed: {e}"),
      }
    }
  }

  #[test]
  fn unverified_sources_earn_no_state() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, conns) = start_listener(&rt);

    // Old behavior under test: the first datagram from ANY source used to
    // allocate a conn-map slot, a socket clone, and a parked thread. These
    // sources never echo a cookie, so they must never appear in the map.
    for i in 0..64u8 {
      let sock = UdpSocket::bind("127.0.0.1:0").expect("bind flood source");
      sock
        .send_to(&[0x16, 0xfe, 0xff, 0x00, i], server)
        .expect("send garbage");
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(
      conns.lock().expect("conn map lock").is_empty(),
      "unverified sources must not allocate connection state"
    );
  }

  #[test]
  fn cookie_verified_handshakes_complete_and_earn_state() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, conns) = start_listener(&rt);

    // An off-the-shelf OpenSSL client only completes against this listener
    // by answering the HelloVerifyRequest, so success here proves the full
    // stateless exchange: HVR off the listener thread, cookie echo,
    // promotion, and the post-listen accept (including its cookie
    // re-verification against the stamped peer address).
    let mut first = connect_client(server);
    assert_eq!(conns.lock().expect("conn map lock").len(), 1);

    // CoAP ping over the promoted session: empty CON must come back RST
    // with the message id echoed, proving the connection thread serves
    // datagrams end to end.
    first
      .ssl_write(&[0x40, 0x00, 0x12, 0x34])
      .expect("ping write");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 16];
    let n = loop {
      match first.ssl_read(&mut buf) {
        Ok(n) => break n,
        Err(e) if e.code() == ErrorCode::WANT_READ => {
          assert!(Instant::now() < deadline, "ping response timed out");
        }
        Err(e) => panic!("ping read failed: {e}"),
      }
    };
    assert_eq!(&buf[..n], &[0x70, 0x00, 0x12, 0x34]);

    // A second client exercises the pending-stream rebuild after a
    // promotion consumed the previous one.
    let _second = connect_client(server);
    assert_eq!(conns.lock().expect("conn map lock").len(), 2);
  }

  fn test_key(byte: u8) -> PKey<Private> {
    PKey::hmac(&[byte; 32]).expect("hmac key")
  }

  fn addr(s: &str) -> SocketAddr {
    s.parse().expect("socket addr")
  }

  #[test]
  fn cookie_round_trips_for_the_same_source() {
    let key = test_key(7);
    for peer in [addr("192.0.2.1:5684"), addr("[2001:db8::1]:5684")] {
      let cookie = generate_cookie(&key, &peer).expect("generate");
      assert!(verify_cookie(&key, &peer, &cookie));
    }
  }

  #[test]
  fn cookie_rejects_a_different_address_port_or_key() {
    let key = test_key(7);
    let peer = addr("192.0.2.1:5684");
    let cookie = generate_cookie(&key, &peer).expect("generate");

    assert!(!verify_cookie(&key, &addr("192.0.2.2:5684"), &cookie));
    assert!(!verify_cookie(&key, &addr("192.0.2.1:5685"), &cookie));
    assert!(!verify_cookie(&test_key(8), &peer, &cookie));
  }

  #[test]
  fn cookie_rejects_truncation_and_garbage() {
    let key = test_key(7);
    let peer = addr("192.0.2.1:5684");
    let cookie = generate_cookie(&key, &peer).expect("generate");

    assert!(!verify_cookie(&key, &peer, &cookie[..cookie.len() - 1]));
    assert!(!verify_cookie(&key, &peer, &[]));
    assert!(!verify_cookie(&key, &peer, &vec![0u8; cookie.len()]));
  }

  #[test]
  fn cookie_material_binds_family_address_and_port() {
    // A v4 address and its v6-mapped form must not share a cookie, and
    // the port must contribute -- collisions here would let a cookie
    // earned at one source be replayed from another.
    let v4 = cookie_material(&addr("192.0.2.1:5684"));
    let mapped = cookie_material(&addr("[::ffff:192.0.2.1]:5684"));
    let other_port = cookie_material(&addr("192.0.2.1:5685"));
    assert_ne!(v4, mapped);
    assert_ne!(v4, other_port);
  }

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
