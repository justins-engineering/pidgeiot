use worker::Env;

/// The Worker var (`[vars]`/`[env.staging.vars]`/`[env.dev.vars]`,
/// `wrangler.toml`) holding the comma-separated pigeon ids allowlisted for
/// the public, unauthenticated demo routes (`GET /demo/pigeons/:id/
/// telemetry*`, `lib.rs`). Empty/unset means no pigeon is demo-eligible --
/// the allowlist's only failure mode is "allow nothing", never "allow
/// anything".
const DEMO_PIGEON_IDS_VAR: &str = "DEMO_PIGEON_IDS";

/// Exact-match check against `DEMO_PIGEON_IDS` -- the sole authorization
/// for the public demo routes, standing in for the Kratos session/ACL
/// check dashboard routes use and the bearer-token check device routes
/// use (a demo visitor has neither). Every demo route handler calls this
/// before touching the pigeon's Durable Object or Postgres data, and
/// returns a plain 404 (not 403) on failure -- don't confirm or deny
/// existence to an unauthenticated caller.
pub fn is_demo_pigeon(env: &Env, pigeon_id: &str) -> bool {
  demo_pigeon_ids(env).iter().any(|id| id == pigeon_id)
}

/// The parsed form of `DEMO_PIGEON_IDS` -- also consulted by the
/// telemetry-history retention sweep (`helpers/retention.rs`) to exclude
/// the demo pigeon's rows from deletion. Same allowlist, same "empty means
/// none" default as `is_demo_pigeon`; reused rather than re-parsed so the
/// two call sites can't drift on what counts as "the demo pigeon".
pub fn demo_pigeon_ids(env: &Env) -> Vec<String> {
  let Ok(raw) = env.var(DEMO_PIGEON_IDS_VAR) else {
    return Vec::new();
  };
  parse_ids(&raw.to_string())
}

fn parse_ids(raw: &str) -> Vec<String> {
  raw
    .split(',')
    .map(str::trim)
    .filter(|id| !id.is_empty())
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
mod tests {
  use super::parse_ids;

  #[test]
  fn splits_trims_and_drops_empties() {
    assert_eq!(parse_ids("a, b ,c"), vec!["a", "b", "c"]);
    assert_eq!(parse_ids("solo"), vec!["solo"]);
  }

  #[test]
  fn empty_or_blank_yields_no_ids() {
    assert_eq!(parse_ids(""), Vec::<String>::new());
    assert_eq!(parse_ids(" , , "), Vec::<String>::new());
  }
}
