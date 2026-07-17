// PENDING #18: dovecote+capsules (owned by the dovecote agent this cycle)
// hasn't landed the telemetry-history routes or the Pigeon.telemetry_endpoint
// field yet. The types below mirror task #18's described shapes as closely
// as possible so the UI has something concrete to build against, but they
// are NOT capsules types — once #18 lands real Row/API types in capsules,
// replace these with the shared ones instead of keeping a parallel
// definition here. Every request path/shape here is a documented guess; if
// the real route lands with a different shape, this module is the only
// place that needs to change.
use crate::api::fetch_json;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::JsFuture;

/// GET /pigeons/:id/telemetry — latest value per key (mirrors the DO's
/// `pigeon_telemetry` table: one row per key, upserted on each device
/// report — see CLAUDE.md's device-facing telemetry ingestion notes).
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct TelemetryLatest {
  pub key: String,
  pub value: String,
  pub reported_at: i64,
}

/// One row of GET /pigeons/:id/telemetry/history or
/// /flocks/:id/telemetry/history (mirrors task #18's described
/// `pigeon_telemetry_history` Postgres table: `value_num` is `None` when
/// `value` didn't parse as a number, e.g. a firmware version string).
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct TelemetryPoint {
  pub key: String,
  pub value: String,
  pub value_num: Option<f64>,
  pub reported_at: i64,
}

/// Same as `TelemetryPoint` but for the flock-scoped route, which spans
/// multiple pigeons and so needs to say which one each row came from.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct FlockTelemetryPoint {
  pub pigeon_id: String,
  pub key: String,
  pub value: String,
  pub value_num: Option<f64>,
  pub reported_at: i64,
}

pub async fn get_latest(pigeon_id: &str) -> Option<Vec<TelemetryLatest>> {
  let mut path = String::with_capacity(96);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// `since_unix`/`until_unix` are unix seconds, matching the wire-size-minded
/// epoch convention `capsules::PigeonShadow` already uses for `updated_at`.
pub async fn get_history(
  pigeon_id: &str,
  since_unix: i64,
  until_unix: i64,
) -> Option<Vec<TelemetryPoint>> {
  let mut path = String::with_capacity(128);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/history?since=");
  path.push_str(&since_unix.to_string());
  path.push_str("&until=");
  path.push_str(&until_unix.to_string());

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

pub async fn get_flock_history(
  flock_id: &uuid::Uuid,
  since_unix: i64,
  until_unix: i64,
) -> Option<Vec<FlockTelemetryPoint>> {
  let mut path = String::with_capacity(128);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());
  path.push_str("/telemetry/history?since=");
  path.push_str(&since_unix.to_string());
  path.push_str("&until=");
  path.push_str(&until_unix.to_string());

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// Mirrors task #18's described `Pigeon.telemetry_endpoint` shape
/// (`{url, db?, auth_token?}`) — `auth_token` is write-only, stripped on
/// read the same way connector tokens are (CLAUDE.md).
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
pub struct TelemetryEndpointRequest {
  pub url: String,
  pub db: Option<String>,
  pub auth_token: Option<String>,
}

pub async fn set_telemetry_endpoint(pigeon_id: &str, req: &TelemetryEndpointRequest) -> Option<()> {
  let mut path = String::with_capacity(96);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/endpoint");

  let json_string = serde_json::to_string(req).ok()?;
  let body = serde_wasm_bindgen::to_value(&json_string).ok()?;
  fetch_json("PUT", &path, Some(&body)).await?;
  Some(())
}

pub async fn clear_telemetry_endpoint(pigeon_id: &str) -> Option<()> {
  let mut path = String::with_capacity(96);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/endpoint");

  fetch_json("DELETE", &path, None).await?;
  Some(())
}
