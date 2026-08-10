//! PSK identity -> secret resolution against dovecote's service-secret-
//! gated internal endpoint (`GET /internal/coap-psk/:identity`), with a
//! short positive/negative cache.
//!
//! Trust chain (documented in docs/infra/coap-terminator.md): the PSK
//! identity is the pigeon's DO id; the lookup yields BOTH the short PSK
//! that keys the handshake and the pigeon's device bearer token (minted
//! together, rotated together -- see `capsules::CoapConfig` for why they
//! are distinct strings). The PSK proves the peer is this pigeon; the
//! token is what the ordinary `/device/pigeons/:id/*` routes require, and
//! the upstream DO still cryptographically verifies it on every proxied
//! request.
//!
//! Staleness window: a `token/refresh` rotates the bearer token AND the
//! PSK together, but a positive cache entry here can let the OLD PSK
//! complete a handshake for up to `positive_ttl` (default 60s) afterwards.
//! That handshake is harmless beyond its own existence: the stale entry's
//! bearer token is revoked, so every upstream call such a session could
//! make 401s at the DO. There is no window in which a revoked credential
//! can read or write data through this terminator.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One resolved credential pair: the PSK that keys the handshake and the
/// bearer token presented upstream on the session's behalf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PskEntry {
  pub psk: String,
  pub token: String,
}

/// Blocking lookup source (the real one is dovecote over HTTP; tests use
/// a closure). Returns Ok(None) for "authoritatively unknown identity"
/// and Err for transport/5xx failures, which are treated as indeterminate
/// rather than negative.
pub trait PskSource: Send + Sync {
  fn fetch(&self, identity: &str) -> Result<Option<PskEntry>, String>;
}

impl<F> PskSource for F
where
  F: Fn(&str) -> Result<Option<PskEntry>, String> + Send + Sync,
{
  fn fetch(&self, identity: &str) -> Result<Option<PskEntry>, String> {
    self(identity)
  }
}

/// Dovecote-backed source. Synchronous by design: DTLS/TLS PSK callbacks
/// are synchronous callbacks invoked mid-handshake; callers on the tokio
/// runtime wrap `PskResolver::resolve` in `tokio::task::block_in_place`.
pub struct DovecotePskSource {
  agent: ureq::Agent,
  base_url: String,
  service_secret: String,
}

impl DovecotePskSource {
  pub fn new(base_url: &str, service_secret: String) -> DovecotePskSource {
    DovecotePskSource {
      agent: ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("loft/", env!("CARGO_PKG_VERSION")))
        .build(),
      base_url: base_url.trim_end_matches('/').to_string(),
      service_secret,
    }
  }
}

impl PskSource for DovecotePskSource {
  fn fetch(&self, identity: &str) -> Result<Option<PskEntry>, String> {
    let url = format!("{}/internal/coap-psk/{}", self.base_url, identity);
    let resp = self
      .agent
      .get(&url)
      .set("Authorization", &format!("Bearer {}", self.service_secret))
      .call();

    match resp {
      Ok(resp) => {
        let lookup: capsules::CoapPskLookup = resp
          .into_json()
          .map_err(|e| format!("psk lookup body parse: {e}"))?;
        Ok(Some(PskEntry {
          psk: lookup.secret,
          token: lookup.token,
        }))
      }
      // 404: known-shape id with no CoAP pigeon behind it. 400: a string
      // that cannot be a pigeon id at all (Durable Object ids carry a
      // namespace check, so dovecote 400s before any lookup). Both are
      // authoritative "no such identity" -- negative-cacheable, so a
      // garbage-identity flood can't bypass the cache.
      Err(ureq::Error::Status(404 | 400, _)) => Ok(None),
      // 401/403 mean OUR service secret is wrong -- indeterminate for the
      // device (and loudly logged by the caller), not a negative entry
      // that would poison the cache for a real identity.
      Err(ureq::Error::Status(status, _)) => Err(format!("psk lookup: upstream {status}")),
      Err(e) => Err(format!("psk lookup transport: {e}")),
    }
  }
}

enum Entry {
  Known { entry: PskEntry, fetched: Instant },
  Unknown { fetched: Instant },
}

pub struct PskResolver {
  source: Box<dyn PskSource>,
  cache: Mutex<HashMap<String, Entry>>,
  positive_ttl: Duration,
  negative_ttl: Duration,
  /// How long a stale positive entry may still be served when the source
  /// is unreachable (availability over freshness for transient dovecote
  /// blips; bounded so a rotated PSK can't linger indefinitely).
  stale_grace: Duration,
}

pub const DEFAULT_POSITIVE_TTL: Duration = Duration::from_secs(60);
pub const DEFAULT_NEGATIVE_TTL: Duration = Duration::from_secs(10);
pub const DEFAULT_STALE_GRACE: Duration = Duration::from_secs(300);

impl PskResolver {
  pub fn new(source: Box<dyn PskSource>, positive_ttl: Duration) -> PskResolver {
    PskResolver {
      source,
      cache: Mutex::new(HashMap::new()),
      positive_ttl,
      negative_ttl: DEFAULT_NEGATIVE_TTL,
      stale_grace: DEFAULT_STALE_GRACE,
    }
  }

  /// Resolves an identity to its credential pair. `None` means "reject
  /// the handshake" (unknown identity, or source unreachable with no
  /// usable stale entry).
  pub fn resolve(&self, identity: &str) -> Option<PskEntry> {
    let now = Instant::now();

    {
      let cache = self.cache.lock().expect("psk cache lock");
      match cache.get(identity) {
        Some(Entry::Known { entry, fetched })
          if now.duration_since(*fetched) < self.positive_ttl =>
        {
          return Some(entry.clone());
        }
        Some(Entry::Unknown { fetched }) if now.duration_since(*fetched) < self.negative_ttl => {
          return None;
        }
        _ => {}
      }
    }

    match self.source.fetch(identity) {
      Ok(Some(entry)) => {
        self.cache.lock().expect("psk cache lock").insert(
          identity.to_string(),
          Entry::Known {
            entry: entry.clone(),
            fetched: now,
          },
        );
        Some(entry)
      }
      Ok(None) => {
        self
          .cache
          .lock()
          .expect("psk cache lock")
          .insert(identity.to_string(), Entry::Unknown { fetched: now });
        None
      }
      Err(e) => {
        tracing::warn!(identity, error = %e, "PSK source unreachable");
        let cache = self.cache.lock().expect("psk cache lock");
        match cache.get(identity) {
          Some(Entry::Known { entry, fetched })
            if now.duration_since(*fetched) < self.stale_grace =>
          {
            tracing::warn!(identity, "serving stale PSK entry (source unreachable)");
            Some(entry.clone())
          }
          _ => None,
        }
      }
    }
  }

  /// Drops one identity's entry -- not called anywhere yet, but the
  /// explicit invalidation hook the documented cache-staleness contract
  /// reserves for a future push-invalidation path.
  #[allow(dead_code)]
  pub fn invalidate(&self, identity: &str) {
    self.cache.lock().expect("psk cache lock").remove(identity);
  }

  #[cfg(test)]
  fn with_ttls(
    source: Box<dyn PskSource>,
    positive_ttl: Duration,
    negative_ttl: Duration,
    stale_grace: Duration,
  ) -> PskResolver {
    PskResolver {
      source,
      cache: Mutex::new(HashMap::new()),
      positive_ttl,
      negative_ttl,
      stale_grace,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::{AtomicUsize, Ordering};

  fn entry(psk: &str) -> PskEntry {
    PskEntry {
      psk: psk.to_string(),
      token: format!("token-{psk}"),
    }
  }

  fn counting_source(
    hits: Arc<AtomicUsize>,
    result: impl Fn(&str) -> Result<Option<PskEntry>, String> + Send + Sync + 'static,
  ) -> Box<dyn PskSource> {
    Box::new(move |identity: &str| {
      hits.fetch_add(1, Ordering::SeqCst);
      result(identity)
    })
  }

  #[test]
  fn positive_lookups_are_cached() {
    let hits = Arc::new(AtomicUsize::new(0));
    let resolver = PskResolver::new(
      counting_source(hits.clone(), |_| Ok(Some(entry("secret1")))),
      Duration::from_secs(60),
    );

    assert_eq!(resolver.resolve("pigeon-a"), Some(entry("secret1")));
    assert_eq!(resolver.resolve("pigeon-a"), Some(entry("secret1")));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn positive_entries_expire() {
    let hits = Arc::new(AtomicUsize::new(0));
    let resolver = PskResolver::with_ttls(
      counting_source(hits.clone(), |_| Ok(Some(entry("s")))),
      Duration::ZERO,
      Duration::ZERO,
      Duration::ZERO,
    );
    resolver.resolve("a");
    resolver.resolve("a");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
  }

  #[test]
  fn negative_lookups_are_cached_briefly() {
    let hits = Arc::new(AtomicUsize::new(0));
    let resolver = PskResolver::new(
      counting_source(hits.clone(), |_| Ok(None)),
      Duration::from_secs(60),
    );
    assert_eq!(resolver.resolve("nope"), None);
    assert_eq!(resolver.resolve("nope"), None);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn source_error_without_stale_entry_rejects() {
    let hits = Arc::new(AtomicUsize::new(0));
    let resolver = PskResolver::new(
      counting_source(hits.clone(), |_| Err("down".into())),
      Duration::from_secs(60),
    );
    assert_eq!(resolver.resolve("a"), None);
    // Errors are not cached -- next attempt hits the source again.
    assert_eq!(resolver.resolve("a"), None);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
  }

  #[test]
  fn source_error_serves_stale_positive_within_grace() {
    let hits = Arc::new(AtomicUsize::new(0));
    let flaky_hits = hits.clone();
    let source = Box::new(move |_: &str| {
      let n = flaky_hits.fetch_add(1, Ordering::SeqCst);
      if n == 0 {
        Ok(Some(entry("orig")))
      } else {
        Err("down".into())
      }
    });
    // positive_ttl zero forces a refetch every call; grace keeps stale.
    let resolver = PskResolver::with_ttls(
      source,
      Duration::ZERO,
      Duration::ZERO,
      Duration::from_secs(300),
    );
    assert_eq!(resolver.resolve("a"), Some(entry("orig")));
    assert_eq!(resolver.resolve("a"), Some(entry("orig")));
    assert_eq!(hits.load(Ordering::SeqCst), 2);
  }

  #[test]
  fn invalidate_forces_refetch() {
    let hits = Arc::new(AtomicUsize::new(0));
    let resolver = PskResolver::new(
      counting_source(hits.clone(), |_| Ok(Some(entry("s")))),
      Duration::from_secs(60),
    );
    resolver.resolve("a");
    resolver.invalidate("a");
    resolver.resolve("a");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
  }
}
