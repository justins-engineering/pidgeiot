//! Env-var-driven configuration, with one exception: the secret. Two
//! deployments read this differently (see docs/infra/coap-terminator.md) --
//! a container gets COAP_SERVICE_SECRET as a plain env var, while the
//! production systemd unit uses LoadCredential= so the value never sits in
//! this process's environment (visible to any code path that can read
//! /proc/self/environ, including a future dependency). `resolve_service_secret`
//! handles both without either deployment needing to know about the other.

use std::path::Path;
use std::time::Duration;

/// Which implementation terminates DTLS on `LOFT_UDP_LISTEN`. The OpenSSL
/// listener is the incumbent; the mbedTLS listener adds RFC 9146
/// Connection ID (see docs/infra/coap-cid-design.md) and stays inert
/// unless selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DtlsStack {
  Openssl,
  Mbedtls,
}

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
  /// LOFT_DTLS_STACK: which stack binds LOFT_UDP_LISTEN.
  pub dtls_stack: DtlsStack,
  /// LOFT_DTLS_MBED_CANARY_ADDR: when set (e.g. 0.0.0.0:5685), an
  /// additional mbedTLS DTLS listener on that address while the primary
  /// stays wherever LOFT_DTLS_STACK points -- same process, same quota,
  /// same resolver; the canary mechanism of the CID rollout.
  pub dtls_mbed_canary_addr: Option<String>,
  /// LOFT_DTLS_CID_IDLE_SECS: idle deadline for CID-negotiated sessions.
  /// Multi-hour PSM sleep gaps are the CID design case, so this is hours
  /// where the non-CID deadline is minutes.
  pub dtls_cid_idle: Duration,
}

/// Default CID-session idle deadline (6h, the recorded owner decision).
const DEFAULT_CID_IDLE: Duration = Duration::from_secs(21_600);

impl Config {
  /// Reads config from the environment. The only hard-required var is
  /// COAP_SERVICE_SECRET (same name as dovecote's Worker secret, on
  /// purpose -- one name, two sides of one credential) -- or, under
  /// systemd, the LoadCredential= file it resolves to. See
  /// `resolve_service_secret`.
  pub fn from_env() -> Result<Config, String> {
    let credentials_dir = std::env::var("CREDENTIALS_DIRECTORY").ok();
    let env_secret = std::env::var("COAP_SERVICE_SECRET").ok();
    let service_secret = resolve_service_secret(
      credentials_dir.as_deref().map(Path::new),
      env_secret.as_deref(),
    )?;
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
      dtls_stack: parse_dtls_stack(std::env::var("LOFT_DTLS_STACK").ok().as_deref())?,
      dtls_mbed_canary_addr: std::env::var("LOFT_DTLS_MBED_CANARY_ADDR")
        .ok()
        .filter(|v| !v.trim().is_empty()),
      dtls_cid_idle: std::env::var("LOFT_DTLS_CID_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_CID_IDLE),
    })
  }
}

/// A typo in the stack selector must refuse to start, not silently serve
/// the wrong implementation.
fn parse_dtls_stack(value: Option<&str>) -> Result<DtlsStack, String> {
  match value {
    None => Ok(DtlsStack::Openssl),
    Some("openssl") => Ok(DtlsStack::Openssl),
    Some("mbedtls") => Ok(DtlsStack::Mbedtls),
    Some(other) => Err(format!(
      "LOFT_DTLS_STACK must be \"openssl\" or \"mbedtls\", got {other:?}"
    )),
  }
}

fn env_or(key: &str, default: &str) -> String {
  std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Resolves the shared dovecote secret, preferring a systemd credential
/// file over the plain env var. `credentials_dir` is
/// $CREDENTIALS_DIRECTORY when systemd set one up for this unit (any
/// LoadCredential=/SetCredential= makes it set the directory itself, even
/// if this particular credential wasn't configured -- so its presence
/// alone doesn't guarantee the file exists); `env_secret` is
/// $COAP_SERVICE_SECRET. Takes both as plain values rather than reading
/// the environment itself so the precedence logic is testable without
/// mutating real process state.
///
/// A credential file wins when both are present -- if a unit is set up
/// with LoadCredential=, that's the deployment's intended source of
/// truth, and honoring a stray env var instead would silently ignore it.
fn resolve_service_secret(
  credentials_dir: Option<&Path>,
  env_secret: Option<&str>,
) -> Result<String, String> {
  if let Some(dir) = credentials_dir {
    let path = dir.join("COAP_SERVICE_SECRET");
    match std::fs::read_to_string(&path) {
      // LoadCredential= copies the source file's bytes verbatim, including
      // a trailing newline if the file that was provisioned has one (e.g.
      // `echo secret > file` rather than `printf`) -- trim it so an
      // operator's shell habits don't become part of the credential value.
      Ok(contents) => return Ok(contents.trim_end_matches('\n').to_string()),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
        // CREDENTIALS_DIRECTORY exists but nothing loaded this specific
        // name -- fall through to the env var below.
      }
      Err(e) => {
        return Err(format!(
          "failed to read credential COAP_SERVICE_SECRET from {}: {e}",
          path.display()
        ));
      }
    }
  }
  env_secret
    .map(str::to_string)
    .ok_or_else(|| "COAP_SERVICE_SECRET is not set (shared secret with dovecote)".to_string())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicU64, Ordering};

  /// A fresh, unique scratch directory per test -- tests run in parallel
  /// in the same process, so a shared fixed path would race.
  fn scratch_dir() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("loft-config-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
  }

  #[test]
  fn dtls_stack_parses_strictly() {
    assert_eq!(parse_dtls_stack(None), Ok(DtlsStack::Openssl));
    assert_eq!(parse_dtls_stack(Some("openssl")), Ok(DtlsStack::Openssl));
    assert_eq!(parse_dtls_stack(Some("mbedtls")), Ok(DtlsStack::Mbedtls));
    let err = parse_dtls_stack(Some("mbed")).expect_err("typos must fail closed");
    assert!(
      err.contains("LOFT_DTLS_STACK"),
      "error names the var: {err}"
    );
  }

  #[test]
  fn credential_file_wins_over_env_var() {
    let dir = scratch_dir();
    std::fs::write(dir.join("COAP_SERVICE_SECRET"), "from-credential\n").expect("write credential");
    let resolved = resolve_service_secret(Some(&dir), Some("from-env")).expect("resolves");
    assert_eq!(
      resolved, "from-credential",
      "credential file takes precedence"
    );
  }

  #[test]
  fn credential_file_trailing_newline_is_trimmed() {
    let dir = scratch_dir();
    std::fs::write(dir.join("COAP_SERVICE_SECRET"), "shhh\n").expect("write credential");
    let resolved = resolve_service_secret(Some(&dir), None).expect("resolves");
    assert_eq!(resolved, "shhh");
  }

  #[test]
  fn missing_credential_file_falls_back_to_env_var() {
    let dir = scratch_dir();
    // CREDENTIALS_DIRECTORY set (systemd unit has *some* LoadCredential=)
    // but not this one -- e.g. only relevant to a future second credential.
    let resolved = resolve_service_secret(Some(&dir), Some("from-env")).expect("falls back");
    assert_eq!(resolved, "from-env");
  }

  #[test]
  fn no_credentials_dir_uses_env_var() {
    let resolved = resolve_service_secret(None, Some("from-env")).expect("resolves");
    assert_eq!(resolved, "from-env");
  }

  #[test]
  fn neither_source_is_an_error() {
    let err = resolve_service_secret(None, None).expect_err("must fail closed");
    assert!(
      err.contains("COAP_SERVICE_SECRET"),
      "error names the missing var: {err}"
    );
  }

  #[test]
  fn credentials_dir_present_but_unreadable_file_is_an_error_not_a_silent_fallback() {
    // A directory in place of the expected file is the simplest way to
    // provoke a non-NotFound io error without touching permissions.
    let dir = scratch_dir();
    std::fs::create_dir(dir.join("COAP_SERVICE_SECRET")).expect("shadow with a directory");
    let err = resolve_service_secret(Some(&dir), Some("from-env"))
      .expect_err("must not silently fall back");
    assert!(
      err.contains("COAP_SERVICE_SECRET"),
      "error identifies the credential that failed to read: {err}"
    );
  }
}
