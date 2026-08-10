//! Shared OpenSSL PSK server configuration for both listeners.
//!
//! PSK, not certificates: RFC 8323 with PSK ciphersuites needs no server
//! certificate at all (the shared key authenticates both sides), matching
//! the `~/pigeon` Zephyr client's `TLS_CREDENTIAL_PSK` setup and the
//! constrained-device norm (no clock, no CA store, no X.509 parsing in a
//! minimal mbedTLS build). TLS is pinned to 1.2: the PSK *ciphersuite*
//! family (TLS_PSK_WITH_AES_128_CCM_8 etc.) is a TLS 1.2 concept, and
//! TLS 1.3's external-PSK story is a different mechanism constrained
//! stacks don't speak yet.

use std::sync::{Arc, LazyLock};

use openssl::error::ErrorStack;
use openssl::ex_data::Index;
use openssl::ssl::{Ssl, SslContext, SslContextBuilder, SslMethod, SslVersion};

use crate::psk::PskResolver;

/// Constrained-device PSK suites, most-preferred first. CCM_8 (8-byte tag)
/// is the CoAP/cellular-IoT standard suite; GCM and CBC variants cover
/// clients built without CCM.
pub const PSK_CIPHER_LIST: &str = "PSK-AES128-CCM8:PSK-AES128-GCM-SHA256:PSK-AES128-CBC-SHA256";

/// After a successful PSK exchange the callback stashes
/// (identity, secret) here so the post-handshake code can build its
/// `DeviceSession` from what was actually authenticated.
pub static SESSION_EX_INDEX: LazyLock<Index<Ssl, (String, String)>> =
  LazyLock::new(|| Ssl::new_ex_index().expect("ssl ex index"));

/// Builds a PSK-only server context for either method (`SslMethod::dtls()`
/// or `SslMethod::tls_server()`).
pub fn build_psk_server_context(
  method: SslMethod,
  is_dtls: bool,
  resolver: Arc<PskResolver>,
) -> Result<SslContextBuilder, ErrorStack> {
  let mut builder = SslContext::builder(method)?;

  if is_dtls {
    builder.set_min_proto_version(Some(SslVersion::DTLS1_2))?;
  } else {
    builder.set_min_proto_version(Some(SslVersion::TLS1_2))?;
    // TLS 1.3 PSK is a different mechanism (psk_key_exchange_modes /
    // session-ticket shaped); cap at 1.2 where the classic PSK
    // ciphersuites live.
    builder.set_max_proto_version(Some(SslVersion::TLS1_2))?;
  }

  builder.set_cipher_list(PSK_CIPHER_LIST)?;

  builder.set_psk_server_callback(move |ssl, identity, psk_out| {
    psk_callback(&resolver, ssl, identity, psk_out)
  });

  Ok(builder)
}

fn psk_callback(
  resolver: &PskResolver,
  ssl: &mut openssl::ssl::SslRef,
  identity: Option<&[u8]>,
  psk_out: &mut [u8],
) -> Result<usize, ErrorStack> {
  // Returning Ok(0) aborts the handshake without a distinguishable
  // "unknown identity" signal to the peer -- deliberate; a probe learns
  // nothing beyond "handshake failed".
  let Some(identity) = identity else {
    return Ok(0);
  };
  let Ok(identity) = std::str::from_utf8(identity) else {
    tracing::debug!("rejecting non-UTF-8 PSK identity");
    return Ok(0);
  };

  // Cached (60s positive TTL) blocking lookup against dovecote. Runs on
  // the connection's own OS thread -- never on the tokio runtime.
  let Some(secret) = resolver.resolve(identity) else {
    tracing::info!(identity, "PSK identity rejected");
    return Ok(0);
  };

  // PSK bytes convention: the raw UTF-8 bytes of the secret string,
  // matching the device side (Zephyr `tls_credential_add(...,
  // TLS_CREDENTIAL_PSK, secret, strlen(secret))` in ~/pigeon).
  let len = secret.len();
  if len > psk_out.len() {
    tracing::error!(identity, "PSK secret longer than OpenSSL's PSK buffer");
    return Ok(0);
  }
  psk_out[..len].copy_from_slice(secret.as_bytes());

  ssl.set_ex_data(*SESSION_EX_INDEX, (identity.to_string(), secret));
  Ok(len)
}

/// Pulls the handshake-authenticated (identity, secret) pair off a
/// completed connection.
pub fn authenticated_session(ssl: &openssl::ssl::SslRef) -> Option<(String, String)> {
  ssl.ex_data(*SESSION_EX_INDEX).cloned()
}
