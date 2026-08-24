use std::net::IpAddr;

use worker::Env;

/// The per-environment var whose value already distinguishes local dev
/// from every deployed environment: the authority devices dial to reach
/// this Worker. Only `[env.dev.vars]` points it at a loopback literal --
/// staging and production both name a real public host. Reused here
/// rather than adding an environment-name var, which would be a second
/// place to keep in sync with the same fact.
const DEVICE_API_HOST_VAR: &str = "DEVICE_API_HOST";

/// Whether this Worker is running under a local `wrangler dev` rather
/// than in a deployed environment.
///
/// Callers use this to decide whether a missing binding is an expected
/// local gap or a deployment fault. It deliberately answers `false` when
/// the var is absent or unparseable: an unrecognised environment must be
/// treated as deployed, so a genuine misconfiguration surfaces as an
/// error instead of silently taking the permissive dev path.
pub fn is_local_dev(env: &Env) -> bool {
  let Ok(host) = env.var(DEVICE_API_HOST_VAR) else {
    return false;
  };
  is_loopback_authority(&host.to_string())
}

/// Accepts the authority forms this var actually carries across the three
/// environments: a bare host, `host:port`, and the bracketed IPv6 spelling
/// of either. A bare IPv6 address without brackets is left whole rather
/// than split on its own colons, so it parses instead of being mangled
/// into a false negative.
fn is_loopback_authority(authority: &str) -> bool {
  let authority = authority.trim();
  let host = if let Some(rest) = authority.strip_prefix('[') {
    rest.split(']').next().unwrap_or(rest)
  } else if authority.matches(':').count() == 1 {
    authority.split(':').next().unwrap_or(authority)
  } else {
    authority
  };

  if host.eq_ignore_ascii_case("localhost") {
    return true;
  }
  host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
  use super::is_loopback_authority;

  #[test]
  fn the_dev_authority_is_recognised_in_every_form_it_takes() {
    assert!(is_loopback_authority("127.0.0.1:8787"));
    assert!(is_loopback_authority("127.0.0.1"));
    assert!(is_loopback_authority("localhost:4455"));
    assert!(is_loopback_authority("LocalHost"));
    assert!(is_loopback_authority(" 127.0.0.1:8787 "));
  }

  #[test]
  fn bracketed_and_bare_ipv6_loopback_both_match() {
    assert!(is_loopback_authority("[::1]:8787"));
    assert!(is_loopback_authority("[::1]"));
    assert!(is_loopback_authority("::1"));
  }

  #[test]
  fn the_whole_127_block_counts_not_just_the_canonical_address() {
    assert!(is_loopback_authority("127.0.0.2"));
    assert!(is_loopback_authority("127.255.255.254:9000"));
  }

  #[test]
  fn deployed_authorities_are_never_local() {
    assert!(!is_loopback_authority("api.pidgeiot.com"));
    assert!(!is_loopback_authority(
      "dovecote-staging.justinsengineeringservices.workers.dev"
    ));
    assert!(!is_loopback_authority("coap.pidgeiot.com"));
    assert!(!is_loopback_authority("15.204.254.3"));
  }

  #[test]
  fn an_unrecognisable_value_is_treated_as_deployed() {
    assert!(!is_loopback_authority(""));
    assert!(!is_loopback_authority("not an authority"));
    assert!(!is_loopback_authority("localhost.evil.example"));
  }
}
