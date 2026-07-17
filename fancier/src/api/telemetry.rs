// Task #18's telemetry-history routes and capsules types landed in dovecote
// (bc1373c) — this module is a thin wrapper around them rather than a
// parallel guess. Setting/clearing a pigeon's telemetry endpoint lives in
// api/pigeons.rs (`update_telemetry_endpoint`) alongside the other
// per-pigeon PUT routes, not here.
use crate::api::fetch_json;
use capsules::{TelemetryHistoryPoint, TelemetryLatest};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use wasm_bindgen_futures::JsFuture;

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

pub async fn get_history(
  pigeon_id: &str,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<Vec<TelemetryHistoryPoint>> {
  let mut path = String::with_capacity(160);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// Same `TelemetryHistoryPoint` shape as `get_history` — the flock-scoped
/// route spans multiple pigeons, but the row already carries `pigeon_id`
/// unconditionally (capsules doesn't have a separate flock-only variant).
pub async fn get_flock_history(
  flock_id: &uuid::Uuid,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<Vec<TelemetryHistoryPoint>> {
  let mut path = String::with_capacity(160);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}
