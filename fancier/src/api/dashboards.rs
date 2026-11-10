//! dovecote's dashboards routes and the two flock-wide reads their widgets poll (docs/api.md's
//! "Dashboards" section). Nothing here caches: a dashboard page builds its state from each
//! write's response, which is the only read guaranteed to reflect that write.

use crate::api::orgs::{error_text, parse, to_body};
use crate::api::{fetch_json, fetch_json_any_status};
use capsules::{
  Dashboard, DashboardCreateRequest, DashboardSummary, DashboardUpdateRequest, FlockShadowVersions,
  MAX_TELEMETRY_BATCH_PIGEONS, PigeonLatestTelemetry, PigeonTelemetryBatchRequest,
};
use uuid::Uuid;

const NOT_FOUND: u16 = 404;
const CONFLICT: u16 = 409;

fn path_for(id: Uuid, suffix: &str) -> String {
  let mut path = String::with_capacity(48 + suffix.len());
  path.push_str("/dashboards/");
  path.push_str(&id.to_string());
  path.push_str(suffix);
  path
}

/// `GET /dashboards`. `None` is a failed request, never an empty account.
pub async fn list() -> Option<Vec<DashboardSummary>> {
  let response = fetch_json("GET", "/dashboards", None).await?;
  parse(response).await
}

/// What a read of one dashboard found.
pub enum DashboardRead {
  Found(Dashboard),
  /// It does not exist, or this account cannot see it; dovecote does not say which.
  Missing,
  Unavailable,
}

/// `GET /dashboards/:dashboard_id`.
pub async fn get(id: Uuid) -> DashboardRead {
  let Some(response) = fetch_json_any_status("GET", &path_for(id, ""), None).await else {
    return DashboardRead::Unavailable;
  };
  if response.status() == NOT_FOUND {
    return DashboardRead::Missing;
  }
  if !response.ok() {
    return DashboardRead::Unavailable;
  }
  match parse(response).await {
    Some(dashboard) => DashboardRead::Found(dashboard),
    None => DashboardRead::Unavailable,
  }
}

/// `POST /dashboards`. `Err` carries dovecote's own message (the cap, a bad name).
pub async fn create(request: &DashboardCreateRequest) -> Result<Dashboard, String> {
  created(fetch_json_any_status("POST", "/dashboards", to_body(request).as_ref()).await).await
}

/// `POST /dashboards/:dashboard_id/copy`: a personal copy of what the caller can read.
pub async fn copy(id: Uuid) -> Result<Dashboard, String> {
  created(fetch_json_any_status("POST", &path_for(id, "/copy"), None).await).await
}

async fn created(response: Option<web_sys::Response>) -> Result<Dashboard, String> {
  let Some(response) = response else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  parse(response)
    .await
    .ok_or_else(|| "Failed to parse response".to_string())
}

/// How a save ended.
pub enum SaveOutcome {
  Saved(Dashboard),
  /// Someone saved since this page loaded; this is what is stored now.
  Conflict(Dashboard),
  Failed(String),
}

/// `PUT /dashboards/:dashboard_id` at the revision the page loaded.
pub async fn save(id: Uuid, request: &DashboardUpdateRequest) -> SaveOutcome {
  let Some(response) =
    fetch_json_any_status("PUT", &path_for(id, ""), to_body(request).as_ref()).await
  else {
    return SaveOutcome::Failed("Network error".to_string());
  };
  if response.status() == CONFLICT {
    return match parse(response).await {
      Some(current) => SaveOutcome::Conflict(current),
      None => SaveOutcome::Failed("Failed to parse response".to_string()),
    };
  }
  if !response.ok() {
    return SaveOutcome::Failed(error_text(&response).await);
  }
  match parse(response).await {
    Some(saved) => SaveOutcome::Saved(saved),
    None => SaveOutcome::Failed("Failed to parse response".to_string()),
  }
}

/// `DELETE /dashboards/:dashboard_id`.
pub async fn delete(id: Uuid) -> Result<(), String> {
  let Some(response) = fetch_json_any_status("DELETE", &path_for(id, ""), None).await else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    Ok(())
  } else {
    Err(error_text(&response).await)
  }
}

/// `POST /pigeons/batch/telemetry`, split into the route's 48-id calls. `None` when any call
/// failed, so a partial answer never reads as a smaller flock. A pigeon missing from a
/// successful answer could not be read.
pub async fn latest_telemetry(
  ids: &[String],
  keys: &[String],
) -> Option<Vec<PigeonLatestTelemetry>> {
  let mut all = Vec::with_capacity(ids.len());
  for chunk in ids.chunks(MAX_TELEMETRY_BATCH_PIGEONS) {
    let body = to_body(&PigeonTelemetryBatchRequest {
      ids: chunk.to_vec(),
      keys: keys.to_vec(),
    })?;
    let response = fetch_json("POST", "/pigeons/batch/telemetry", Some(&body)).await?;
    let answer: Vec<PigeonLatestTelemetry> = parse(response).await?;
    all.extend(answer);
  }
  Some(all)
}

/// `GET /flocks/:flock_id/shadow/versions`, with `keys` as its comma list.
pub async fn shadow_versions(flock_id: Uuid, keys: &[String]) -> Option<FlockShadowVersions> {
  let joined = keys.join(",");
  let mut path = String::with_capacity(64 + joined.len());
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());
  path.push_str("/shadow/versions");
  if !joined.is_empty() {
    path.push_str("?keys=");
    path.push_str(&js_sys::encode_uri_component(&joined).as_string()?);
  }
  let response = fetch_json("GET", &path, None).await?;
  parse(response).await
}
