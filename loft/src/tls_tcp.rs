//! CoAP-over-TLS/TCP listener (coaps+tcp, 5684/tcp, RFC 8323) -- the
//! secondary transport; what the `~/pigeon` Zephyr client speaks today.
//! Same PSK story and thread-per-connection model as the DTLS listener.
//!
//! RFC 8323 messaging notes:
//! - We send our CSM (7.01) immediately after the handshake, per section
//!   5.3. We tolerate peers that never send theirs -- the minimal
//!   `~/pigeon` client sends requests directly, and rejecting it would be
//!   standards-compliant but pointless.
//! - Ping (7.02) gets Pong (7.03) with the same token; Release/Abort end
//!   the connection.
//! - No message ids, no ACKs, no retransmission -- TCP owns reliability.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use openssl::ssl::{HandshakeError, Ssl, SslContext, SslMethod, SslStream};

use crate::coap::message::{Message, code};
use crate::coap::tcp::{FrameDecoder, encode_frame};
use crate::config::Config;
use crate::handler::{DeviceSession, Handler, Transport};
use crate::psk::PskResolver;
use crate::tls_common::{authenticated_session, build_psk_server_context};
use crate::upstream::Dovecote;

const MAX_CONNECTIONS: usize = 4096;
/// One blocking read tick; idle handling rides on TCP read timeouts.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

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

  let live = Arc::new(AtomicUsize::new(0));

  for stream in listener.incoming() {
    let stream = match stream {
      Ok(s) => s,
      Err(e) => {
        tracing::warn!(error = %e, "accept failed");
        continue;
      }
    };

    if live.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
      tracing::warn!("connection cap reached, refusing TCP peer");
      continue;
    }

    let ctx = ctx.clone();
    let handler = handler.clone();
    let rt = rt.clone();
    live.fetch_add(1, Ordering::SeqCst);
    let live_in_thread = live.clone();
    let spawned = std::thread::Builder::new()
      .name("coaps-tcp-conn".into())
      .spawn(move || {
        connection_thread(&ctx, stream, &handler, &rt);
        live_in_thread.fetch_sub(1, Ordering::SeqCst);
      });
    if spawned.is_err() {
      live.fetch_sub(1, Ordering::SeqCst);
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
  if tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err() || tcp.set_nodelay(true).is_err() {
    return;
  }

  let ssl = match Ssl::new(ctx) {
    Ok(s) => s,
    Err(e) => {
      tracing::error!(error = %e, "Ssl::new failed");
      return;
    }
  };

  let mut stream = match ssl.accept(tcp) {
    Ok(s) => s,
    Err(HandshakeError::Failure(mid)) => {
      tracing::info!(%peer, error = %mid.error(), "TLS handshake failed");
      return;
    }
    Err(e) => {
      tracing::info!(%peer, error = %e, "TLS handshake failed");
      return;
    }
  };

  let Some((identity, secret)) = authenticated_session(stream.ssl()) else {
    tracing::error!(%peer, "handshake completed without an authenticated identity");
    return;
  };
  tracing::info!(%peer, identity, "coaps+tcp session established");

  let _ = stream.get_ref().set_read_timeout(Some(IDLE_TIMEOUT));

  let session = DeviceSession {
    pigeon_id: identity,
    secret,
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
  stream: &mut SslStream<TcpStream>,
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
