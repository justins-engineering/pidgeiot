// Task #32's dovecote alert routes (docs/api.md, dovecote/src/lib.rs's
// "Alert Routes" section) -- CRUD for user-defined `capsules::AlertDefinition`s.
// Named `alerts.rs`, not `alert.rs`, matching the plural convention already
// used by `pigeons.rs`/`flocks.rs` in this directory; `capsules::AlertDefinition`
// itself is named to avoid colliding with the unrelated toast
// `components::Alert`/`models::AlertVariant` (see docs/design/alerts-triggers.md
// §0) -- this module never imports either of those.
use crate::api::fetch_json;
use capsules::{AlertDefinition, AlertDefinitionCreateRequest, AlertDefinitionUpdateRequest};
use dioxus::prelude::*;
use uuid::Uuid;
use wasm_bindgen_futures::JsFuture;

/// Shared by every function below -- inserts/updates one alert in
/// `LocalSession.alerts` (keyed by id, same additive-cache convention as
/// `api::pigeons`/`api::flocks`).
fn cache(alert: &AlertDefinition) {
  let mut alerts = consume_context::<crate::LocalSession>().alerts;
  alerts.insert(alert.id, alert.clone());
  alerts.write();
}

/// `POST /pigeons/:pigeon_id/alerts` -- scope is implied by the route, not
/// part of the request body (see `AlertDefinitionCreateRequest`'s doc
/// comment in capsules).
pub async fn create_pigeon(
  pigeon_id: &str,
  req: &AlertDefinitionCreateRequest,
) -> Option<AlertDefinition> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/alerts");

  let body = serde_json::to_string(req).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let alert = serde_wasm_bindgen::from_value::<AlertDefinition>(json).ok()?;
  cache(&alert);
  Some(alert)
}

/// `GET /pigeons/:pigeon_id/alerts` -- every alert scoped directly to this
/// pigeon (not flock-scoped alerts that happen to apply to it -- callers
/// wanting both render `PigeonAlerts` alongside the pigeon's flock's own
/// `FlockAlerts` section, mirroring how `PigeonGraphs`/`FlockGraphs` are two
/// separate sections rather than one merged view).
pub async fn list_pigeon(pigeon_id: &str) -> Option<Vec<AlertDefinition>> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/alerts");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let alerts = serde_wasm_bindgen::from_value::<Vec<AlertDefinition>>(json).ok()?;
  for alert in &alerts {
    cache(alert);
  }
  Some(alerts)
}

/// `POST /flocks/:flock_id/alerts` -- flock-owner-gated server-side (only
/// the flock owner can create a flock-scoped alert, unlike per-pigeon
/// alerts which any ACL'd user can create for a pigeon they can access).
pub async fn create_flock(
  flock_id: Uuid,
  req: &AlertDefinitionCreateRequest,
) -> Option<AlertDefinition> {
  let path = format!("/flocks/{flock_id}/alerts");

  let body = serde_json::to_string(req).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let alert = serde_wasm_bindgen::from_value::<AlertDefinition>(json).ok()?;
  cache(&alert);
  Some(alert)
}

/// `GET /flocks/:flock_id/alerts` -- every alert scoped to this flock.
pub async fn list_flock(flock_id: Uuid) -> Option<Vec<AlertDefinition>> {
  let path = format!("/flocks/{flock_id}/alerts");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let alerts = serde_wasm_bindgen::from_value::<Vec<AlertDefinition>>(json).ok()?;
  for alert in &alerts {
    cache(alert);
  }
  Some(alerts)
}

/// `PUT /alerts/:alert_id` -- owner-gated regardless of scope (an alert's
/// `user_id` is unambiguous whether it's pigeon- or flock-scoped, see
/// dovecote's `is_alert_owner`). Used both for full edits and for the
/// list view's inline enabled/disabled toggle (only `enabled` set, the
/// rest left `None` to keep their current values per the request's own
/// partial-update convention).
pub async fn update(alert_id: Uuid, req: &AlertDefinitionUpdateRequest) -> Option<AlertDefinition> {
  let path = format!("/alerts/{alert_id}");

  let body = serde_json::to_string(req).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let alert = serde_wasm_bindgen::from_value::<AlertDefinition>(json).ok()?;
  cache(&alert);
  Some(alert)
}

/// `DELETE /alerts/:alert_id`.
pub async fn delete(alert_id: Uuid) -> Option<()> {
  let path = format!("/alerts/{alert_id}");

  let _response = fetch_json("DELETE", &path, None).await?;
  let mut alerts = consume_context::<crate::LocalSession>().alerts;
  alerts.remove(&alert_id);
  alerts.write();
  Some(())
}
