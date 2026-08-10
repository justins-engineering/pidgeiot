//! loft -- PidgeIoT's CoAP terminator. Two listeners on port 5684 (one
//! UDP/DTLS, one TCP/TLS), both PSK-authenticated against per-pigeon
//! credentials resolved from dovecote, translating the CoAP device surface
//! onto dovecote's HTTP device routes. See docs/infra/coap-terminator.md.
//!
//! Concurrency model (deliberate, boring): both listeners are
//! thread-per-connection with blocking OpenSSL IO. PSK callbacks are
//! synchronous C callbacks invoked mid-handshake; running every connection
//! on a plain thread means the callback can do its (cached) blocking HTTP
//! lookup directly -- no sync-over-async bridging anywhere in the
//! handshake path. The tokio runtime exists solely for the upstream
//! reqwest client; connection threads enter it via `Handle::block_on` per
//! request. At single-VPS scale (hundreds-to-low-thousands of mostly
//! sleeping devices) threads are nowhere near a bottleneck, and the
//! transport is isolated behind `handler::Handler` so a poll-driven or
//! sans-IO backend can replace a listener without touching the CoAP layer.

mod coap;
mod config;
mod dtls;
mod handler;
mod psk;
mod tls_common;
mod tls_tcp;
mod upstream;

use std::sync::Arc;

use crate::config::Config;
use crate::handler::Handler;
use crate::psk::{DovecotePskSource, PskResolver};
use crate::upstream::Dovecote;

fn main() -> anyhow::Result<()> {
  tracing_subscriber::fmt()
    .with_env_filter(
      tracing_subscriber::EnvFilter::try_from_env("LOFT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    )
    .init();

  let config = Config::from_env().map_err(|e| anyhow::anyhow!(e))?;

  let runtime = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(2)
    .enable_all()
    .build()?;

  let resolver = Arc::new(PskResolver::new(
    Box::new(DovecotePskSource::new(
      &config.dovecote_url,
      config.service_secret.clone(),
    )),
    config.psk_cache_ttl,
  ));

  let upstream = Dovecote::new(&config.dovecote_url).map_err(|e| anyhow::anyhow!(e))?;
  let handler = Arc::new(Handler::new(upstream));

  tracing::info!(
    udp = %config.udp_listen,
    tcp = %config.tcp_listen,
    upstream = %config.dovecote_url,
    "loft starting"
  );

  let udp = {
    let config = config.clone();
    let resolver = resolver.clone();
    let handler = handler.clone();
    let rt = runtime.handle().clone();
    std::thread::Builder::new()
      .name("dtls-listener".into())
      .spawn(move || dtls::run(&config, resolver, handler, rt))?
  };

  let tcp = {
    let resolver = resolver.clone();
    let handler = handler.clone();
    let rt = runtime.handle().clone();
    std::thread::Builder::new()
      .name("tls-listener".into())
      .spawn(move || tls_tcp::run(&config, resolver, handler, rt))?
  };

  // Either listener exiting is fatal -- the container restarts us.
  let udp_result = udp.join();
  let tcp_result = tcp.join();
  tracing::error!(?udp_result, ?tcp_result, "listener exited");
  anyhow::bail!("listener exited");
}
