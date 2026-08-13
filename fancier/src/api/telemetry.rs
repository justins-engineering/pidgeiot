// Task #18's telemetry-history routes and capsules types landed in dovecote
// (bc1373c) — this module is a thin wrapper around them rather than a
// parallel guess. Setting/clearing a pigeon's telemetry endpoint lives in
// api/pigeons.rs (`update_telemetry_endpoint`) alongside the other
// per-pigeon PUT routes, not here.
use crate::api::fetch_json;
use capsules::{TELEMETRY_HISTORY_TRUNCATED_HEADER, TelemetryHistoryPoint, TelemetryLatest};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use wasm_bindgen_futures::JsFuture;
use web_sys::Response;

/// `TelemetryHistoryQuery` (capsules) deserializes `since`/`until` via
/// `time::serde::rfc3339::option`, so the wire format is an RFC3339 string,
/// not a raw unix integer. `:` is the only reserved character an
/// RFC3339-with-`Z`-offset timestamp (always the case for
/// `OffsetDateTime::now_utc()`) ever contains, so a full percent-encoding
/// crate would be overkill — hand-replacing it is enough to keep the query
/// string valid.
fn rfc3339_query_value(t: OffsetDateTime) -> String {
  t.format(&Rfc3339).unwrap_or_default().replace(':', "%3A")
}

/// GET /pigeons/:id/telemetry — latest value per key (mirrors the DO's
/// `pigeon_telemetry` table: one row per key, upserted on each device
/// report — see CLAUDE.md's device-facing telemetry ingestion notes).
pub async fn get_latest(pigeon_id: &str) -> Option<Vec<TelemetryLatest>> {
  let mut path = String::with_capacity(96);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// A history read together with whether the server had to cut the range
/// short. Both facts come from the same response — the points are the
/// body, the cap is `TELEMETRY_HISTORY_TRUNCATED_HEADER` — so callers get
/// them as one value rather than one of them being available only to
/// whoever remembers to look.
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryHistory {
  pub points: Vec<TelemetryHistoryPoint>,
  /// `None` means the response said nothing either way (a backend older
  /// than the header). Deliberately not folded into `false`: "we did not
  /// ask" and "this range is complete" are different claims, and only the
  /// second one is safe to render as a complete window.
  pub truncated: Option<bool>,
}

/// The header is exposed cross-origin by dovecote's `build_cors`
/// (`with_exposed_headers`), so this read works from the browser; without
/// that it would silently always be `None`.
fn truncated_header(response: &Response) -> Option<bool> {
  let raw = response
    .headers()
    .get(TELEMETRY_HISTORY_TRUNCATED_HEADER)
    .ok()
    .flatten()?;
  match raw.trim() {
    "true" => Some(true),
    "false" => Some(false),
    _ => None,
  }
}

/// Shared by both history routes below — identical response handling, only
/// the path differs.
async fn fetch_history(path: &str) -> Option<TelemetryHistory> {
  let response = fetch_json("GET", path, None).await?;
  let truncated = truncated_header(&response);
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  Some(TelemetryHistory {
    points: serde_wasm_bindgen::from_value(json).ok()?,
    truncated,
  })
}

pub async fn get_history(
  pigeon_id: &str,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<TelemetryHistory> {
  let mut path = String::with_capacity(160);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  fetch_history(&path).await
}

/// Same `TelemetryHistoryPoint` shape as `get_history` — the flock-scoped
/// route spans multiple pigeons, but the row already carries `pigeon_id`
/// unconditionally (capsules doesn't have a separate flock-only variant).
pub async fn get_flock_history(
  flock_id: &uuid::Uuid,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<TelemetryHistory> {
  let mut path = String::with_capacity(160);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  fetch_history(&path).await
}
