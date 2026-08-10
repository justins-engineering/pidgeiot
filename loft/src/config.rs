//! Env-var-driven configuration. No config files, no CLI flags -- the
//! deployment story is a container with env vars (see
//! docs/infra/coap-terminator.md), stateless across restarts.

use std::time::Duration;

#[derive(Clone)]
pub struct Config {
  /// UDP (DTLS, coaps) listen address. Default 0.0.0.0:5684.
  pub udp_listen: String,
  /// TCP (TLS, coaps+tcp) listen address. Default 0.0.0.0:5684 -- same
  /// port, different protocol, per RFC 7252/8323 registered port 5684.
  pub tcp_listen: String,
  /// Dovecote base URL, e.g. https://api.pidgeiot.com (prod) or
  /// http://127.0.0.1:8787 (dev wrangler).
  pub dovecote_url: String,
  /// Shared service secret gating dovecote's /internal/coap-psk/:identity.
  /// Same value as dovecote's COAP_SERVICE_SECRET Worker secret.
  pub service_secret: String,
  /// Positive PSK cache TTL.
  pub psk_cache_ttl: Duration,
}

impl Config {
  /// Reads config from the environment. The only hard-required var is
  /// COAP_SERVICE_SECRET (same name as dovecote's Worker secret, on
  /// purpose -- one name, two sides of one credential).
  pub fn from_env() -> Result<Config, String> {
    let service_secret = std::env::var("COAP_SERVICE_SECRET")
      .map_err(|_| "COAP_SERVICE_SECRET is not set (shared secret with dovecote)".to_string())?;
    if service_secret.trim().is_empty() {
      return Err("COAP_SERVICE_SECRET is empty".to_string());
    }

    Ok(Config {
      udp_listen: env_or("LOFT_UDP_LISTEN", "0.0.0.0:5684"),
      tcp_listen: env_or("LOFT_TCP_LISTEN", "0.0.0.0:5684"),
      dovecote_url: env_or("LOFT_DOVECOTE_URL", "https://api.pidgeiot.com")
        .trim_end_matches('/')
        .to_string(),
      service_secret,
      psk_cache_ttl: std::env::var("LOFT_PSK_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(crate::psk::DEFAULT_POSITIVE_TTL),
    })
  }
}

fn env_or(key: &str, default: &str) -> String {
  std::env::var(key).unwrap_or_else(|_| default.to_string())
}
