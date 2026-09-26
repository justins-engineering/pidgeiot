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

/// The Worker var holding the comma-separated source addresses allowed to post ThingSpace
/// callbacks: Verizon's published callback addresses, in the one environment that holds the
/// `NiddService` registration. Its own var, never `COAP_SERVICE_ALLOWED_IPS`, which also opens
/// the PSK route and exempts the terminator from the failed-auth limiter. Empty or unset denies
/// every caller.
const THINGSPACE_CALLBACK_ALLOWED_IPS_VAR: &str = "THINGSPACE_CALLBACK_ALLOWED_IPS";

/// Whether this environment admits ThingSpace callbacks at all: its allowlist names at least one
/// address. Only that environment may send NIDD downlinks, since Verizon allows one callback
/// endpoint per service per account and a second sender could push shadows to a device whose
/// reports go elsewhere.
pub fn thingspace_callbacks_configured(env: &Env) -> bool {
  env
    .var(THINGSPACE_CALLBACK_ALLOWED_IPS_VAR)
    .is_ok_and(|raw| names_an_address(&raw.to_string()))
}

/// Source-address gate on the ThingSpace callback route, the first of its three gates. The
/// addresses are cloud addresses on a list Verizon calls changing, so this is a filter, not a
/// secret: the callback password is checked after it.
pub fn is_allowed_thingspace_ip(env: &Env, req: &Request) -> bool {
  is_allowed_by(env, req, THINGSPACE_CALLBACK_ALLOWED_IPS_VAR)
}

/// Whether an allowlist admits anyone at all.
fn names_an_address(raw: &str) -> bool {
  raw
    .split(',')
    .any(|entry| entry.trim().parse::<IpAddr>().is_ok())
}

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
  is_allowed_by(env, req, COAP_SERVICE_ALLOWED_IPS_VAR)
}

/// Whether `CF-Connecting-IP` appears in the allowlist held by the var `var`.
fn is_allowed_by(env: &Env, req: &Request, var: &str) -> bool {
  let Ok(raw) = env.var(var) else {
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
  use super::{allowlist_matches, names_an_address};

  const VERIZON_CALLBACK_IPS: &str = "137.117.33.109,168.62.173.153,3.87.163.45,3.91.119.203,\
    54.197.62.209,35.165.205.14,54.200.43.232,34.216.81.234";

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
  fn the_thingspace_list_admits_exactly_verizons_addresses() {
    assert!(names_an_address(VERIZON_CALLBACK_IPS));
    for peer in VERIZON_CALLBACK_IPS.split(',') {
      assert!(allowlist_matches(VERIZON_CALLBACK_IPS, peer.trim()));
    }
    assert!(!allowlist_matches(VERIZON_CALLBACK_IPS, "3.87.163.46"));
    assert!(!names_an_address(""));
    assert!(!names_an_address(" , not-an-ip"));
  }

  #[test]
  fn bad_entries_are_dropped_not_fatal() {
    assert!(allowlist_matches("not-an-ip, 15.204.254.3", "15.204.254.3"));
  }
}
