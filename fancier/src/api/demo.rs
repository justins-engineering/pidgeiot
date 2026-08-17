// Public, unauthenticated demo routes (dovecote's GET /demo/pigeons/:id/
// telemetry*, docs/api.md's "Public Demo API" section) -- backs the public
// `/demo` page (views/demo.rs). Thin wrappers mirroring
// api/telemetry.rs's get_latest/get_history, hitting the unauthenticated
// /demo/... path against a single, build-time-fixed pigeon id
// (config::DEMO_PIGEON_ID) instead of an arbitrary one supplied by a
// signed-in caller. Deliberately does NOT write into LocalSession -- this
// data isn't scoped to any signed-in user's cache, and the demo page never
// touches Session state either (see views/demo.rs). Reuses the shared
// `fetch_json` (api/helpers.rs), which always sends `credentials: include`;
// that's harmless here since dovecote's demo routes never check
// X-User-Id/ACL/cookie either way (helpers::is_demo_pigeon is the only
// gate, dovecote's src/helpers/demo.rs).
use crate::api::fetch_json;
use crate::config::DEMO_PIGEON_ID;
use capsules::{DemoAlert, TelemetryHistoryBucket, TelemetryLatest};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use wasm_bindgen_futures::JsFuture;

/// Same encoding rationale as api/telemetry.rs's identical helper: an
/// RFC3339-with-`Z`-offset timestamp only ever needs `:` escaped for a
/// valid query string.
fn rfc3339_query_value(t: OffsetDateTime) -> String {
  t.format(&Rfc3339).unwrap_or_default().replace(':', "%3A")
}

/// GET /demo/pigeons/:id/telemetry -- latest value per key for the fixed
/// demo pigeon. `None` on any fetch/parse failure OR when `DEMO_PIGEON_ID`
/// is empty (unset in this environment, e.g. dev -- dovecote would 404
/// with an empty path segment anyway, but views/demo.rs checks this first
/// so it never fires the request at all).
pub async fn get_latest() -> Option<Vec<TelemetryLatest>> {
  if DEMO_PIGEON_ID.is_empty() {
    return None;
  }

  let mut path = String::with_capacity(64);
  path.push_str("/demo/pigeons/");
  path.push_str(DEMO_PIGEON_ID);
  path.push_str("/telemetry");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// GET /demo/pigeons/:id/telemetry/history -- bucketed by default (no
/// `raw=true`), same as `api::telemetry::get_history_buckets`. The demo
/// pigeon (5 keys reported every 30s) is the case that most directly
/// motivated bucketing: under the old flat/truncating shape, its 6h
/// history request only ever drew ~3.5h before hitting
/// `TELEMETRY_HISTORY_MAX_POINTS`.
pub async fn get_history(
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<Vec<TelemetryHistoryBucket>> {
  if DEMO_PIGEON_ID.is_empty() {
    return None;
  }

  let mut path = String::with_capacity(128);
  path.push_str("/demo/pigeons/");
  path.push_str(DEMO_PIGEON_ID);
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// GET /demo/pigeons/:id/alerts -- the alert definitions governing the demo
/// pigeon, as `DemoAlert`: a deliberately narrow projection that never
/// carries the owning user's id or the notification channel (which holds an
/// email address). Pigeon-scoped and enabled-only, so what comes back is
/// exactly the set of rules actually being evaluated against the readings
/// on the page.
pub async fn get_alerts() -> Option<Vec<DemoAlert>> {
  if DEMO_PIGEON_ID.is_empty() {
    return None;
  }

  let mut path = String::with_capacity(64);
  path.push_str("/demo/pigeons/");
  path.push_str(DEMO_PIGEON_ID);
  path.push_str("/alerts");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}
