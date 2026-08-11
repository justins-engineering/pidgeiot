use std::net::IpAddr;

use worker::{Env, Request};

/// The Worker var (`[vars]`/`[env.staging.vars]`/`[env.dev.vars]`,
/// `wrangler.toml`) holding the comma-separated source addresses allowed
/// to call the service-internal PSK route (`GET
/// /internal/coap-psk/:pigeon_id`, `lib.rs`): the egress addresses of the
/// CoAP terminator (`loft`), the route's only legitimate caller.
/// Empty/unset means no caller is allowed -- the allowlist's only failure
/// mode is "allow nothing", never "allow anything" (same convention as
/// `DEMO_PIGEON_IDS`).
const COAP_SERVICE_ALLOWED_IPS_VAR: &str = "COAP_SERVICE_ALLOWED_IPS";

/// Network gate layered ahead of the `COAP_SERVICE_SECRET` check on the
/// internal PSK route. The secret alone grants unscoped PSK resolution
/// for every pigeon, so a leaked copy must not be usable from anywhere
/// but the terminator host itself. Compares `CF-Connecting-IP` -- set by
/// Cloudflare's edge on every path into a deployed Worker (custom domain
/// and workers.dev traffic cannot bypass the edge, which overwrites any
/// client-supplied value; `wrangler dev` populates it with the local
/// client's address) -- against the allowlist. A missing or unparseable
/// header denies; an unparseable allowlist entry is dropped, which can
/// only ever shrink what's allowed, never widen it.
pub fn is_allowed_coap_service_ip(env: &Env, req: &Request) -> bool {
  let Ok(raw) = env.var(COAP_SERVICE_ALLOWED_IPS_VAR) else {
    return false;
  };
  let Some(peer) = req.headers().get("CF-Connecting-IP").ok().flatten() else {
    return false;
  };
  allowlist_matches(&raw.to_string(), &peer)
}

/// Both sides are parsed as `IpAddr` -- textual variants of one address
/// (IPv6 case, zero compression) must not produce a false mismatch -- and
/// canonicalized so an IPv4-mapped IPv6 peer matches its plain v4
/// allowlist entry.
fn allowlist_matches(raw: &str, peer: &str) -> bool {
  let Ok(peer) = peer.trim().parse::<IpAddr>() else {
    return false;
  };
  let peer = canonical(peer);
  raw
    .split(',')
    .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
    .any(|allowed| canonical(allowed) == peer)
}

/// `::ffff:a.b.c.d` is the same peer as `a.b.c.d`; a runtime that ever
/// presents the client address in mapped form must not false-deny the one
/// legitimate caller. `to_ipv4_mapped` touches only that exact form
/// (never `::1` or any other v6 address, unlike the looser `to_ipv4`), so
/// normalization can only unify equivalent addresses, never widen the
/// list.
fn canonical(addr: IpAddr) -> IpAddr {
  match addr {
    IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(addr, IpAddr::V4),
    IpAddr::V4(_) => addr,
  }
}

#[cfg(test)]
mod tests {
  use super::allowlist_matches;

  #[test]
  fn matches_exact_v4_and_v6_entries() {
    assert!(allowlist_matches("15.204.254.3", "15.204.254.3"));
    assert!(allowlist_matches("127.0.0.1,::1", "::1"));
    assert!(allowlist_matches(" 127.0.0.1 , ::1 ", "127.0.0.1"));
  }

  #[test]
  fn ipv4_mapped_peer_matches_plain_v4_entry() {
    assert!(allowlist_matches("127.0.0.1", "::ffff:127.0.0.1"));
    assert!(allowlist_matches("::ffff:15.204.254.3", "15.204.254.3"));
  }

  #[test]
  fn normalization_never_widens() {
    assert!(!allowlist_matches("127.0.0.1", "::1"));
    assert!(!allowlist_matches("::1", "127.0.0.1"));
    assert!(!allowlist_matches("127.0.0.1", "0.0.0.1"));
  }

  #[test]
  fn denies_on_empty_garbage_or_mismatch() {
    assert!(!allowlist_matches("", "127.0.0.1"));
    assert!(!allowlist_matches("not-an-ip", "127.0.0.1"));
    assert!(!allowlist_matches("15.204.254.3", "15.204.254.4"));
    assert!(!allowlist_matches("15.204.254.3", "garbage"));
  }

  #[test]
  fn bad_entries_are_dropped_not_fatal() {
    assert!(allowlist_matches("not-an-ip, 15.204.254.3", "15.204.254.3"));
  }
}
