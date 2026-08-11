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
  let Ok(raw) = env.var(DEMO_PIGEON_IDS_VAR) else {
    return false;
  };
  raw
    .to_string()
    .split(',')
    .map(str::trim)
    .any(|id| !id.is_empty() && id == pigeon_id)
}
