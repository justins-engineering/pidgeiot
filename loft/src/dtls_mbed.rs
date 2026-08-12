//! CoAP-over-DTLS/UDP listener on system mbedTLS 3.6, adding RFC 9146
//! Connection ID so a PSM'd device's NAT rebind is one routed datagram
//! instead of a timeout and a re-handshake. Same skeleton as the OpenSSL
//! listener (`dtls.rs`): one listener thread demultiplexes an unconnected
//! socket into per-connection channels, each connection runs blocking on
//! its own thread, and everything above the decrypted-datagram boundary is
//! the shared `dtls_common` layer. Design and decision record:
//! docs/infra/coap-cid-design.md.
//!
//! What CID changes about the demux: an established session is reachable
//! by the 8-byte Connection ID its uplink records carry (content type 25),
//! not only by source address -- so `ConnMap` becomes two maps. A CID
//! record whose CID is unknown is dropped silently with a rate-limited
//! counter: it can never begin a handshake, must never reach the cookie
//! path, and answering it would be an amplification primitive.
//!
//! The pre-auth posture is the OpenSSL listener's, verbatim: an unverified
//! source owns no map entry, no channel, no thread -- one long-lived
//! pending session serves the stateless HelloVerifyRequest exchange
//! through mbedTLS's own cookie module (which replaces the hand-rolled
//! HMAC cookie code; same HMAC-over-claimed-address construction, with
//! built-in timed key rotation). mbedTLS reports both "garbage discarded"
//! and "cookie-verified ClientHello consumed, reply flight written" as
//! WANT_READ; the shim's recording cookie-check wrapper is what tells
//! promotion apart from noise.
//!
//! Address migration is authenticated-read-gated (RFC 9146 section 6):
//! CIDs are plaintext on the wire, so nothing about routing or the reply
//! path moves on datagram arrival -- only after `mbedtls_ssl_read`
//! returns data (AEAD plus the default anti-replay window passed) does
//! the session compare the datagram's staged source against its committed
//! peer and commit the change. The accepted residual (an off-path
//! attacker racing a captured, not-yet-delivered record) is the RFC's
//! own: self-healing on the next authentic record, no confidentiality
//! impact.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mbedtls_ffi_shim::{
  CID_LEN, Config as MbedConfig, HandshakeStatus, MbedIo, ReadStatus, RecvOutcome, ResolvedPsk,
  SendOutcome, Session, TimerState,
};

use crate::dtls_common::{DedupCache, process_datagram, rand_u16};
use crate::handler::{DeviceSession, Handler, next_conn_id};
use crate::psk::PskResolver;
use crate::quota::{ConnPermit, ConnQuota};
use crate::tls_common::resolve_psk_identity;
use crate::upstream::Dovecote;

const CONN_CHANNEL_DEPTH: usize = 32;
const READ_TICK: Duration = Duration::from_secs(1);
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
/// Idle deadline for sessions that did NOT negotiate CID -- the current
/// fleet's shape, kept bit-for-bit at the OpenSSL listener's figure.
const IDLE_DEADLINE: Duration = Duration::from_secs(300);
const DTLS_MTU: u16 = 1400;
/// How often the unknown-CID drop counter may emit a log line.
const UNKNOWN_CID_LOG_PERIOD: Duration = Duration::from_secs(60);

/// tls12_cid content type (RFC 9146).
const CONTENT_TYPE_CID: u8 = 25;
/// Plaintext handshake content type.
const CONTENT_TYPE_HANDSHAKE: u8 = 22;

/// One promoted session's demux presence.
#[derive(Clone)]
struct SessionHandle {
  tx: SyncSender<(Vec<u8>, SocketAddr)>,
  /// Set once the handshake completes; the demux guard that lets a reused
  /// source address start a fresh handshake past a still-mapped session.
  established: Arc<AtomicBool>,
  /// Guard for removal: entries are only ever removed by the session that
  /// owns them, so a slot re-keyed to a newer session is never clobbered.
  conn_id: u64,
}

#[derive(Default)]
struct Maps {
  by_addr: HashMap<SocketAddr, SessionHandle>,
  by_cid: HashMap<[u8; CID_LEN], SessionHandle>,
}

type ConnMap = Arc<Mutex<Maps>>;

/// The transport a session owns: reads pop whole datagrams (with their
/// source) from the demux channel, writes are pinned to the committed
/// peer. Doubles as the pending session's one-shot feed on the listener
/// thread.
struct ConnState {
  sock: UdpSocket,
  rx: Receiver<(Vec<u8>, SocketAddr)>,
  /// Where replies go. Changes only through an authenticated migration
  /// commit in the session loop, never on datagram arrival.
  committed_peer: SocketAddr,
  /// Source of the most recently delivered datagram; compared against the
  /// committed peer only after an authenticated read.
  staged_peer: SocketAddr,
  /// One-shot injection slot for the pending-listen path (the pending
  /// session's datagrams never ride the channel).
  pending: Option<(Vec<u8>, SocketAddr)>,
  /// Zero while the listener thread owns the session (it must never park
  /// on one source's silence), one READ_TICK on a connection thread.
  tick: Duration,
  /// Handshake wall-clock bound, enforced here as well as in the
  /// handshake loop so a peer feeding valid fragments fast enough to keep
  /// reads busy still cannot dodge it. Armed at promotion, cleared once
  /// established.
  deadline: Option<Instant>,
}

impl ConnState {
  /// Commits a staged source change, returning the new peer. Called only
  /// after an authenticated read.
  fn take_migration(&mut self) -> Option<SocketAddr> {
    (self.staged_peer != self.committed_peer).then(|| {
      self.committed_peer = self.staged_peer;
      self.committed_peer
    })
  }
}

impl MbedIo for ConnState {
  fn send(&mut self, buf: &[u8]) -> SendOutcome {
    match self.sock.send_to(buf, self.committed_peer) {
      Ok(n) => SendOutcome::Sent(n),
      Err(e) => {
        tracing::debug!(error = %e, "send_to failed");
        SendOutcome::Failed
      }
    }
  }

  fn recv(&mut self, buf: &mut [u8], timer: &TimerState) -> RecvOutcome {
    if let Some(deadline) = self.deadline
      && Instant::now() >= deadline
    {
      return RecvOutcome::Failed;
    }
    let (dgram, src) = match self.pending.take() {
      Some(fed) => fed,
      None => {
        // Park no longer than the shortest of: the tick (deadline-check
        // cadence), the retransmission timer (whose expiry mbedTLS must
        // hear about as TIMEOUT to resend its flight), and the handshake
        // deadline.
        let mut budget = self.tick;
        if let Some(remaining) = timer.remaining() {
          budget = budget.min(remaining);
        }
        if let Some(deadline) = self.deadline {
          budget = budget.min(deadline.saturating_duration_since(Instant::now()));
        }
        match self.rx.recv_timeout(budget) {
          Ok(x) => x,
          Err(RecvTimeoutError::Timeout) => {
            return if timer.final_expired() {
              RecvOutcome::TimerExpired
            } else {
              RecvOutcome::WantRead
            };
          }
          Err(RecvTimeoutError::Disconnected) => return RecvOutcome::Closed,
        }
      }
    };
    if dgram.len() > buf.len() {
      // Whole-datagram drop, never truncation: a datagram prefix is a
      // corrupt record, not a short read.
      tracing::debug!(len = dgram.len(), "dropping oversized datagram");
      return RecvOutcome::WantRead;
    }
    buf[..dgram.len()].copy_from_slice(&dgram);
    self.staged_peer = src;
    RecvOutcome::Data(dgram.len())
  }
}

pub fn run(
  listen: &str,
  cid_idle: Duration,
  resolver: Arc<PskResolver>,
  handler: Arc<Handler<Dovecote>>,
  rt: tokio::runtime::Handle,
  quota: ConnQuota,
) {
  if let Err(e) = run_inner(listen, cid_idle, resolver, handler, rt, quota) {
    tracing::error!(error = %e, "mbedTLS DTLS listener failed");
  }
}

fn run_inner(
  listen: &str,
  cid_idle: Duration,
  resolver: Arc<PskResolver>,
  handler: Arc<Handler<Dovecote>>,
  rt: tokio::runtime::Handle,
  quota: ConnQuota,
) -> anyhow::Result<()> {
  let config = build_config(resolver)?;
  let sock = UdpSocket::bind(listen)?;
  tracing::info!(
    addr = %listen,
    mbedtls = %mbedtls_ffi_shim::runtime_version(),
    "DTLS/UDP listener up (mbedTLS, CID)"
  );
  let maps = ConnMap::default();
  listen_loop(sock, &config, &maps, &quota, &handler, &rt, cid_idle)
}

fn build_config(resolver: Arc<PskResolver>) -> anyhow::Result<Arc<MbedConfig>> {
  let resolve: mbedtls_ffi_shim::PskCallback = Box::new(move |identity: &[u8]| {
    let (identity, entry) = resolve_psk_identity(&resolver, identity)?;
    Some(ResolvedPsk {
      // PSK bytes convention: the raw UTF-8 bytes of the secret string,
      // matching the device side and the OpenSSL listener.
      psk: entry.psk.into_bytes(),
      identity,
      token: entry.token,
    })
  });
  MbedConfig::server(resolve)
    .map(Arc::new)
    .map_err(|e| anyhow::anyhow!("mbedTLS server config: {e}"))
}

/// Extracts the server CID from a well-formed tls12_cid record: the
/// demux's whole view of RFC 9146. Fixed offset 11 = type(1) +
/// version(2) + epoch(2) + seq(6); the length is the compile-time
/// `CID_LEN` this listener's config negotiates, so parser and
/// `mbedtls_ssl_conf_cid` cannot disagree. Callers classify by content
/// type first -- a type-25 runt or wrong-version record is still
/// CID-space traffic (it can never begin a handshake) and must be
/// dropped, never allowed to fall through to the pending cookie path,
/// where each one would burn a pending-session reset.
fn cid_of(dgram: &[u8]) -> Option<[u8; CID_LEN]> {
  if dgram.len() >= 21 && dgram[1] == 0xFE && dgram[2] == 0xFD {
    let mut cid = [0u8; CID_LEN];
    cid.copy_from_slice(&dgram[11..11 + CID_LEN]);
    Some(cid)
  } else {
    None
  }
}

/// A plaintext (epoch-0) handshake record -- what a rebooted or
/// replacement device handed a still-mapped ip:port opens with.
fn is_plaintext_handshake(dgram: &[u8]) -> bool {
  dgram.len() >= 5 && dgram[0] == CONTENT_TYPE_HANDSHAKE && dgram[3] == 0 && dgram[4] == 0
}

/// Byte encoding of the claimed source address for the cookie binding;
/// the two families cannot collide (4+2 vs 16+2 bytes).
fn transport_id(peer: &SocketAddr, out: &mut [u8; 18]) -> usize {
  let ip_len = match peer.ip() {
    IpAddr::V4(ip) => {
      out[..4].copy_from_slice(&ip.octets());
      4
    }
    IpAddr::V6(ip) => {
      out[..16].copy_from_slice(&ip.octets());
      16
    }
  };
  out[ip_len..ip_len + 2].copy_from_slice(&peer.port().to_be_bytes());
  ip_len + 2
}

/// The single not-yet-verified session every unknown source is funneled
/// through; consumed by promotion, rebuilt lazily for the next stranger.
struct PendingListen {
  session: Session<ConnState>,
  tx: SyncSender<(Vec<u8>, SocketAddr)>,
}

impl PendingListen {
  fn new(config: &Arc<MbedConfig>, sock: &UdpSocket) -> anyhow::Result<PendingListen> {
    let (tx, rx) = std::sync::mpsc::sync_channel(CONN_CHANNEL_DEPTH);
    let placeholder: SocketAddr = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);
    let io = ConnState {
      sock: sock.try_clone()?,
      rx,
      // Overwritten before any datagram is fed; never sent to as-is.
      committed_peer: placeholder,
      staged_peer: placeholder,
      pending: None,
      tick: Duration::ZERO,
      deadline: None,
    };
    let session =
      Session::new(config, io).map_err(|e| anyhow::anyhow!("pending session setup: {e}"))?;
    Ok(PendingListen { session, tx })
  }
}

/// Mints a CID no live session already owns. The single listener thread
/// mints and promotes serially, so at most one unpromoted CID exists at a
/// time and a mint-time check suffices -- no reservation set.
fn mint_cid(maps: &ConnMap) -> anyhow::Result<[u8; CID_LEN]> {
  let m = maps.lock().expect("conn maps lock");
  loop {
    let mut cid = [0u8; CID_LEN];
    openssl::rand::rand_bytes(&mut cid)?;
    if !m.by_cid.contains_key(&cid) {
      return Ok(cid);
    }
  }
}

fn remove_by_addr_if_mine(maps: &ConnMap, peer: SocketAddr, conn_id: u64) {
  let mut m = maps.lock().expect("conn maps lock");
  if m.by_addr.get(&peer).is_some_and(|h| h.conn_id == conn_id) {
    m.by_addr.remove(&peer);
  }
}

fn remove_by_cid_if_mine(maps: &ConnMap, cid: &[u8; CID_LEN], conn_id: u64) {
  let mut m = maps.lock().expect("conn maps lock");
  if m.by_cid.get(cid).is_some_and(|h| h.conn_id == conn_id) {
    m.by_cid.remove(cid);
  }
}

fn listen_loop(
  sock: UdpSocket,
  config: &Arc<MbedConfig>,
  maps: &ConnMap,
  quota: &ConnQuota,
  handler: &Arc<Handler<Dovecote>>,
  rt: &tokio::runtime::Handle,
  cid_idle: Duration,
) -> anyhow::Result<()> {
  let mut pending: Option<PendingListen> = None;
  let mut buf = vec![0u8; 65535];
  let mut unknown_cid_drops: u64 = 0;
  let mut unknown_cid_logged = Instant::now();

  loop {
    let (len, peer) = match sock.recv_from(&mut buf) {
      Ok(x) => x,
      Err(e) => {
        tracing::warn!(error = %e, "recv_from failed");
        continue;
      }
    };
    let dgram = buf[..len].to_vec();

    // Route 1: everything content-type 25, by CID only. A type-25 record
    // can never begin a handshake, so anything here that doesn't route --
    // unknown CID, runt, wrong version -- is dropped with nothing sent
    // back and nothing allocated (answering would be an amplification
    // primitive, and letting it fall through would burn a pending-session
    // reset per packet). A stale post-restart session lands here and
    // recovers through its own timeout + re-handshake.
    if !dgram.is_empty() && dgram[0] == CONTENT_TYPE_CID {
      let mut m = maps.lock().expect("conn maps lock");
      match cid_of(&dgram).and_then(|cid| m.by_cid.contains_key(&cid).then_some(cid)) {
        Some(cid) => {
          let h = m.by_cid.get(&cid).expect("checked entry");
          match h.tx.try_send((dgram, peer)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
              tracing::debug!(%peer, "connection channel full, dropping datagram");
            }
            Err(TrySendError::Disconnected(_)) => {
              // The mapped session is gone; a dead channel can only
              // belong to the entry's own session (a newer session would
              // carry a live handle), so the entry goes directly.
              m.by_cid.remove(&cid);
            }
          }
        }
        None => {
          unknown_cid_drops += 1;
          if unknown_cid_logged.elapsed() >= UNKNOWN_CID_LOG_PERIOD {
            tracing::debug!(
              count = unknown_cid_drops,
              "dropped unroutable tls12_cid datagrams"
            );
            unknown_cid_drops = 0;
            unknown_cid_logged = Instant::now();
          }
        }
      }
      continue;
    }

    // Route 2: everything else, by exact source address.
    let mut unrouted = Some(dgram);
    {
      let mut m = maps.lock().expect("conn maps lock");
      if let Some(h) = m.by_addr.get(&peer) {
        let d = unrouted.take().expect("datagram present");
        if is_plaintext_handshake(&d) && h.established.load(Ordering::SeqCst) {
          // A fresh ClientHello against an established session: the
          // source address was reused (device reboot or NAT handing the
          // mapping to someone new). Let it earn a new session through
          // the cookie path instead of feeding -- and deafening -- the
          // old one. During a lossy in-flight handshake `established` is
          // still false, so retransmitted ClientHellos keep routing to
          // the session that owns them.
          unrouted = Some(d);
        } else {
          match h.tx.try_send((d, peer)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
              // Backpressure: drop the datagram, UDP semantics.
              tracing::debug!(%peer, "connection channel full, dropping datagram");
            }
            Err(TrySendError::Disconnected(returned)) => {
              // The connection thread is gone; this source is unknown
              // again and must re-earn its slot through the cookie
              // exchange.
              m.by_addr.remove(&peer);
              unrouted = Some(returned.0);
            }
          }
        }
      }
    }
    let Some(dgram) = unrouted else {
      continue;
    };

    // Route 3: the stateless pending path. Full table: don't spend the
    // cookie exchange on sources that could not be admitted anyway (the
    // per-IP share is charged post-cookie, on a proven address).
    if quota.is_full() {
      tracing::warn!(%peer, "connection cap reached, dropping new peer");
      continue;
    }

    if pending.is_none() {
      match PendingListen::new(config, &sock) {
        Ok(pl) => pending = Some(pl),
        Err(e) => {
          tracing::error!(error = %e, "pending listen session setup failed");
          continue;
        }
      }
    }
    let pl = pending.as_mut().expect("pending session present");

    // Each unknown-source datagram gets a clean context: without the
    // reset, a fragmented ClientHello from one source could interleave
    // with bytes from another inside the shared pending context.
    if pl.session.reset().is_err() {
      pending = None;
      continue;
    }
    pl.session.set_mtu(DTLS_MTU);
    {
      let io = pl.session.io_mut();
      io.committed_peer = peer;
      io.staged_peer = peer;
      io.pending = Some((dgram, peer));
    }
    let mut tid = [0u8; 18];
    let tid_len = transport_id(&peer, &mut tid);
    let cid = match mint_cid(maps) {
      Ok(cid) => cid,
      Err(e) => {
        tracing::error!(error = %e, "CID mint failed");
        continue;
      }
    };
    if pl.session.set_client_transport_id(&tid[..tid_len]).is_err()
      || pl.session.set_own_cid(&cid).is_err()
    {
      pending = None;
      continue;
    }

    config.clear_cookie_verified();
    match pl.session.handshake() {
      // The HelloVerifyRequest already went out through f_send straight
      // to the claimed source; nothing allocated, nothing held.
      HandshakeStatus::HelloVerifyRequired => {}
      HandshakeStatus::WantRead | HandshakeStatus::WantWrite => {
        if config.take_cookie_verified() {
          // A cookie-verified ClientHello was consumed and the PSK
          // flight 2 was already written inline -- promote. The per-IP
          // share is charged only now, on a proven address.
          let pl = pending.take().expect("pending session present");
          match quota.try_acquire(peer.ip()) {
            Some(permit) => promote(pl, cid, permit, peer, maps, handler, rt, cid_idle),
            None => {
              tracing::warn!(%peer, "connection quota reached, refusing verified peer");
            }
          }
        }
        // Plain WANT_READ without the flag: garbage, silently discarded
        // by the library; the next attempt resets anyway.
      }
      HandshakeStatus::Done => {
        // A handshake cannot complete off one datagram; treat as a
        // poisoned context.
        tracing::error!(%peer, "pending handshake completed unexpectedly");
        pending = None;
      }
      HandshakeStatus::Failed(e) => {
        tracing::debug!(%peer, error = %e, "pending listen step failed");
      }
    }
  }
}

/// Turns the cookie-verified pending session into a real connection: only
/// now does the source own map entries and an OS thread. The by_cid route
/// must exist before the session thread ever reads -- the client's
/// Finished is already an epoch-1, CID-bearing record.
#[allow(clippy::too_many_arguments)]
fn promote(
  pl: PendingListen,
  cid: [u8; CID_LEN],
  permit: ConnPermit,
  peer: SocketAddr,
  maps: &ConnMap,
  handler: &Arc<Handler<Dovecote>>,
  rt: &tokio::runtime::Handle,
  cid_idle: Duration,
) {
  let PendingListen { mut session, tx } = pl;
  let conn_id = next_conn_id();
  let established = Arc::new(AtomicBool::new(false));
  let handle = SessionHandle {
    tx,
    established: established.clone(),
    conn_id,
  };
  {
    let mut m = maps.lock().expect("conn maps lock");
    m.by_addr.insert(peer, handle.clone());
    m.by_cid.insert(cid, handle);
  }
  {
    let io = session.io_mut();
    io.tick = READ_TICK;
    io.deadline = Some(Instant::now() + HANDSHAKE_DEADLINE);
  }

  let maps_for_thread = maps.clone();
  let handler = handler.clone();
  let rt = rt.clone();
  let spawned = std::thread::Builder::new()
    .name(format!("dtls-{peer}"))
    .spawn(move || {
      // The permit rides the connection thread so every exit path
      // releases its quota slots; a failed spawn drops the closure and
      // settles the same way.
      let _permit = permit;
      // Map cleanup must also survive a panic anywhere in the session
      // path -- an unwinding thread would otherwise strand a dead demux
      // entry that only a later datagram from the same key (or nothing,
      // for a device that never returns) lazily reaps.
      let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session_thread(
          session,
          peer,
          cid,
          conn_id,
          &established,
          &maps_for_thread,
          &handler,
          &rt,
          cid_idle,
        );
      }));
      remove_by_addr_if_mine(&maps_for_thread, peer, conn_id);
      remove_by_cid_if_mine(&maps_for_thread, &cid, conn_id);
      if outcome.is_err() {
        tracing::error!(%peer, "session thread panicked");
      }
    });
  if let Err(e) = spawned {
    tracing::error!(error = %e, "connection thread spawn failed");
    remove_by_addr_if_mine(maps, peer, conn_id);
    remove_by_cid_if_mine(maps, &cid, conn_id);
  }
}

#[allow(clippy::too_many_arguments)]
fn session_thread(
  mut session: Session<ConnState>,
  peer: SocketAddr,
  cid: [u8; CID_LEN],
  conn_id: u64,
  established: &Arc<AtomicBool>,
  maps: &ConnMap,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
  cid_idle: Duration,
) {
  let started = Instant::now();
  loop {
    match session.handshake() {
      HandshakeStatus::Done => break,
      HandshakeStatus::WantRead | HandshakeStatus::WantWrite => {
        if started.elapsed() > HANDSHAKE_DEADLINE {
          tracing::debug!(%peer, "handshake deadline exceeded");
          return;
        }
      }
      HandshakeStatus::HelloVerifyRequired => {
        // The cookie was verified before promotion and renegotiation is
        // off; reaching this is a bug, not a peer behavior.
        tracing::error!(%peer, "unexpected HelloVerifyRequired after promotion");
        return;
      }
      HandshakeStatus::Failed(e) => {
        tracing::info!(%peer, error = %e, "DTLS handshake failed");
        return;
      }
    }
  }
  session.io_mut().deadline = None;

  let cid_active = session.peer_cid().map(|s| s.negotiated).unwrap_or(false);
  let Some((identity, token)) = session.take_credentials() else {
    // Unreachable with PSK-only ciphersuites, but never serve a session
    // whose identity we can't name.
    tracing::error!(%peer, "handshake completed without an authenticated identity");
    return;
  };
  established.store(true, Ordering::SeqCst);
  if cid_active {
    tracing::info!(
      %peer,
      identity,
      cid = %hex(&cid),
      "DTLS session established (CID negotiated)"
    );
  } else {
    // The current fleet's shape: address-routed for life, short idle.
    remove_by_cid_if_mine(maps, &cid, conn_id);
    tracing::info!(%peer, identity, "DTLS session established");
  }

  let mut dev = DeviceSession {
    pigeon_id: identity,
    token,
    peer: peer.to_string(),
    conn_id,
  };
  let idle = if cid_active { cid_idle } else { IDLE_DEADLINE };
  let mut dedup = DedupCache::new();
  let mut next_mid: u16 = rand_u16();
  let mut last_activity = Instant::now();
  let mut buf = vec![0u8; 65535];
  // The address-keyed route is retained until the first authenticated
  // CID-routed read: the client's epoch-0 flight retransmits still route
  // by address if our final flight was lost.
  let mut addr_routed = true;

  loop {
    match session.read(&mut buf) {
      ReadStatus::Data(n) => {
        last_activity = Instant::now();
        if cid_active {
          if addr_routed {
            // From here the session is CID-only and the 5-tuple is free
            // for other devices.
            remove_by_addr_if_mine(maps, peer, conn_id);
            addr_routed = false;
          }
          if let Some(new_peer) = session.io_mut().take_migration() {
            // Authenticated-read-gated commit; a subsequent authentic
            // record from the original address flips it back.
            tracing::info!(
              identity = %dev.pigeon_id,
              from = %dev.peer,
              to = %new_peer,
              "address migration"
            );
            dev.peer = new_peer.to_string();
          }
        }
        if n == 0 {
          continue;
        }
        if let Some(reply) =
          process_datagram(&buf[..n], &dev, handler, rt, &mut dedup, &mut next_mid)
          && let Err(e) = session.write(&reply)
        {
          tracing::debug!(error = %e, "DTLS write failed");
          break;
        }
      }
      ReadStatus::WantRead | ReadStatus::WantWrite => {
        if last_activity.elapsed() > idle {
          tracing::debug!(peer = %dev.peer, "idle timeout");
          break;
        }
      }
      ReadStatus::PeerClosed => break,
      ReadStatus::Failed(e) => {
        tracing::debug!(error = %e, "DTLS read failed");
        break;
      }
    }
  }
  tracing::debug!(peer = %dev.peer, "DTLS session closed");
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::psk::PskEntry;
  use crate::quota::{MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP};

  const TEST_IDENTITY: &str = "test-pigeon";
  const TEST_PSK: &str = "0123456789abcdef0123456789abcdef";

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
    let config = build_config(resolver).expect("server config");
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind listener");
    let addr = sock.local_addr().expect("listener addr");
    let maps = ConnMap::default();
    let quota = ConnQuota::new(MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP);
    let handler = Arc::new(Handler::new(
      Dovecote::new("http://127.0.0.1:9").expect("upstream stub"),
    ));
    let loop_maps = maps.clone();
    let handle = rt.handle().clone();
    std::thread::spawn(move || {
      let _ = listen_loop(
        sock,
        &config,
        &loop_maps,
        &quota,
        &handler,
        &handle,
        Duration::from_secs(21_600),
      );
    });
    (addr, maps)
  }

  /// Client transport over a real (connected) loopback socket, capturing
  /// outbound datagrams so tests can replay them from other sources.
  struct ClientIo {
    sock: UdpSocket,
    captured: Vec<Vec<u8>>,
  }

  impl ClientIo {
    fn new(server: SocketAddr) -> ClientIo {
      let sock = UdpSocket::bind("127.0.0.1:0").expect("bind client");
      sock.connect(server).expect("connect client");
      sock
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("read timeout");
      ClientIo {
        sock,
        captured: Vec::new(),
      }
    }
  }

  impl MbedIo for ClientIo {
    fn send(&mut self, buf: &[u8]) -> SendOutcome {
      self.captured.push(buf.to_vec());
      match self.sock.send(buf) {
        Ok(n) => SendOutcome::Sent(n),
        Err(_) => SendOutcome::Failed,
      }
    }
    fn recv(&mut self, buf: &mut [u8], timer: &TimerState) -> RecvOutcome {
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

  fn connect_client(server: SocketAddr, offer_cid: bool) -> Session<ClientIo> {
    let config = Arc::new(
      MbedConfig::client(TEST_IDENTITY.as_bytes(), TEST_PSK.as_bytes(), offer_cid)
        .expect("client config"),
    );
    let mut session = Session::new(&config, ClientIo::new(server)).expect("client session");
    if offer_cid {
      session.offer_zero_length_cid().expect("cid offer");
    }
    complete_client_handshake(&mut session);
    session
  }

  fn complete_client_handshake(session: &mut Session<ClientIo>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
      match session.handshake() {
        HandshakeStatus::Done => return,
        HandshakeStatus::WantRead | HandshakeStatus::WantWrite => {
          assert!(Instant::now() < deadline, "client handshake timed out");
        }
        HandshakeStatus::HelloVerifyRequired => panic!("client got HVR status"),
        HandshakeStatus::Failed(e) => panic!("client handshake failed: {e}"),
      }
    }
  }

  /// CoAP ping over the session: empty CON must come back RST with the
  /// message id echoed, proving the server serves datagrams end to end.
  fn coap_ping(session: &mut Session<ClientIo>, mid: u16) {
    let [hi, lo] = mid.to_be_bytes();
    assert_eq!(session.write(&[0x40, 0x00, hi, lo]).expect("ping write"), 4);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 32];
    loop {
      match session.read(&mut buf) {
        ReadStatus::Data(n) => {
          assert_eq!(&buf[..n], &[0x70, 0x00, hi, lo], "RST echoing the mid");
          return;
        }
        ReadStatus::WantRead | ReadStatus::WantWrite => {
          assert!(Instant::now() < deadline, "ping response timed out");
        }
        ReadStatus::PeerClosed => panic!("peer closed during ping"),
        ReadStatus::Failed(e) => panic!("ping read failed: {e}"),
      }
    }
  }

  #[test]
  fn unverified_sources_earn_no_state() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, maps) = start_listener(&rt);

    for i in 0..64u8 {
      let sock = UdpSocket::bind("127.0.0.1:0").expect("bind flood source");
      sock
        .send_to(&[0x16, 0xfe, 0xff, 0x00, i], server)
        .expect("send garbage");
    }
    // Unknown-CID records are the other pre-auth surface: silent drop,
    // and type-25 runts stay in CID-space instead of reaching the
    // pending cookie path.
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind cid source");
    let mut cid_record = vec![25, 0xfe, 0xfd, 0, 1, 0, 0, 0, 0, 0, 0, 0, 8];
    cid_record.extend_from_slice(&[0xAB; 8]);
    sock.send_to(&cid_record, server).expect("send cid garbage");
    sock
      .send_to(&[25, 0xfe, 0xfd, 0, 1, 0, 0], server)
      .expect("send cid runt");

    std::thread::sleep(Duration::from_millis(300));
    let m = maps.lock().expect("conn maps lock");
    assert!(m.by_addr.is_empty(), "no address entries for strangers");
    assert!(m.by_cid.is_empty(), "no CID entries for strangers");
  }

  #[test]
  fn cid_session_survives_source_rebind() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, maps) = start_listener(&rt);

    let mut client = connect_client(server, true);
    let cid_view = client.peer_cid().expect("peer cid");
    assert!(cid_view.negotiated, "server must negotiate CID");
    assert_eq!(cid_view.peer_cid.len(), CID_LEN);

    coap_ping(&mut client, 0x1234);
    {
      // After the first authenticated read the session is CID-only.
      let m = maps.lock().expect("conn maps lock");
      assert_eq!(m.by_cid.len(), 1);
      assert!(
        m.by_addr.is_empty(),
        "the 5-tuple must be free for other devices"
      );
    }

    // The rebind: a brand-new source socket, same DTLS session. The next
    // exchange must be one routed datagram -- no re-handshake, which
    // `coap_ping` proves by completing without any handshake() call.
    let fresh = UdpSocket::bind("127.0.0.1:0").expect("bind rebind socket");
    fresh.connect(server).expect("connect rebind socket");
    fresh
      .set_read_timeout(Some(Duration::from_millis(50)))
      .expect("read timeout");
    client.io_mut().sock = fresh;

    coap_ping(&mut client, 0x5678);
    let m = maps.lock().expect("conn maps lock");
    assert_eq!(m.by_cid.len(), 1, "still exactly one session");
    assert!(m.by_addr.is_empty());
  }

  #[test]
  fn no_cid_client_is_served_and_stays_address_routed() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, maps) = start_listener(&rt);

    let mut client = connect_client(server, false);
    let cid_view = client.peer_cid().expect("peer cid");
    assert!(!cid_view.negotiated, "current fleet shape");
    coap_ping(&mut client, 0x2222);

    let m = maps.lock().expect("conn maps lock");
    assert_eq!(m.by_addr.len(), 1, "address-routed for life");
    assert!(m.by_cid.is_empty(), "provisional CID entry removed");
  }

  #[test]
  fn replayed_uplink_is_dropped_and_answers_nothing() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, _maps) = start_listener(&rt);

    let mut client = connect_client(server, true);
    coap_ping(&mut client, 0x0101);

    // Replay the client's last authenticated uplink datagram from a third
    // address: the anti-replay window drops it before any read returns,
    // so the reply path must not move and the spoof source must get
    // nothing back (a reply would be an amplification primitive).
    let replayed = client
      .io()
      .captured
      .iter()
      .rev()
      .find(|d| d[0] == CONTENT_TYPE_CID)
      .expect("captured a CID uplink record")
      .clone();
    let spoof = UdpSocket::bind("127.0.0.1:0").expect("bind spoof");
    spoof
      .set_read_timeout(Some(Duration::from_millis(300)))
      .expect("read timeout");
    spoof.send_to(&replayed, server).expect("send replay");

    let mut buf = [0u8; 64];
    assert!(
      spoof.recv(&mut buf).is_err(),
      "replayed record must draw no response"
    );
    // The original client is still served on its own path.
    coap_ping(&mut client, 0x0202);
  }

  #[test]
  fn reused_source_address_can_rehandshake_past_an_established_session() {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .build()
      .expect("runtime");
    let (server, maps) = start_listener(&rt);

    // A no-CID session keeps its by_addr entry for life -- the shape the
    // reuse lockout used to bite.
    let mut first = connect_client(server, false);
    coap_ping(&mut first, 0x3333);

    // A "rebooted device": a fresh ClientHello from the exact same
    // ip:port while the old session is still established. The demux must
    // route it to the cookie path instead of deafening it against the old
    // session.
    let same_socket = first.io().sock.try_clone().expect("clone socket");
    same_socket
      .set_read_timeout(Some(Duration::from_millis(50)))
      .expect("read timeout");
    let config = Arc::new(
      MbedConfig::client(TEST_IDENTITY.as_bytes(), TEST_PSK.as_bytes(), false)
        .expect("client config"),
    );
    let mut second = Session::new(
      &config,
      ClientIo {
        sock: same_socket,
        captured: Vec::new(),
      },
    )
    .expect("second client session");
    complete_client_handshake(&mut second);
    coap_ping(&mut second, 0x4444);

    let m = maps.lock().expect("conn maps lock");
    assert_eq!(
      m.by_addr.len(),
      1,
      "the reused address is keyed to the new session"
    );
  }
}
