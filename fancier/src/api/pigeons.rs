use crate::api::fetch_json;
use capsules::{
  Connector, Pigeon, PigeonCreateRequest, PigeonDetail, PigeonShadow, PigeonShadowUpdateRequest,
  PigeonTelemetryEndpointUpdateRequest, PigeonUpdateRequest, TelemetryEndpoint,
};
use dioxus::prelude::*;
use std::collections::HashMap;
use wasm_bindgen_futures::JsFuture;

pub async fn list(pigeon_ids: &[String]) -> Option<()> {
  let body = serde_json::to_string(pigeon_ids).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", "/pigeons/batch", Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let pigeons_array = serde_wasm_bindgen::from_value::<Vec<Pigeon>>(json).ok()?;
  let pigeons_map: HashMap<String, Pigeon> = pigeons_array
    .into_iter()
    .map(|pigeon| (pigeon.id.clone(), pigeon))
    .collect();

  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.extend(pigeons_map);
  pigeon_list.write();
  Some(())
}

pub async fn get(pigeon_id: &str) -> Option<String> {
  let mut path = String::with_capacity(73);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  let pigeon = serde_wasm_bindgen::from_value::<Pigeon>(json).ok()?;
  let id = pigeon.id.clone();
  pigeon_list.insert(id.clone(), pigeon);
  pigeon_list.write();
  Some(id)
}

pub async fn get_detail(pigeon_id: &str) -> Option<PigeonDetail> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/detail");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let detail = serde_wasm_bindgen::from_value::<PigeonDetail>(json).ok()?;

  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.insert(detail.pigeon.id.clone(), detail.pigeon.clone());
  pigeon_list.write();

  Some(detail)
}

pub async fn update(pigeon_id: &str, pur: &PigeonUpdateRequest) -> Option<String> {
  let mut path = String::with_capacity(73);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);

  let body = serde_json::to_string(pur).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  let pigeon = serde_wasm_bindgen::from_value::<Pigeon>(json).ok()?;
  let id = pigeon.id.clone();
  pigeon_list.insert(id.clone(), pigeon);
  pigeon_list.write();
  Some(id)
}

pub async fn create(pigeon: &PigeonCreateRequest) -> Option<(String, String)> {
  let body = serde_json::to_string(pigeon).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", "/flock/pigeons", Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  let detail = serde_wasm_bindgen::from_value::<PigeonDetail>(json).ok()?;
  let id = detail.pigeon.id.clone();

  // Cache the pigeon (token is stripped on subsequent GETs)
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.insert(id.clone(), detail.pigeon.clone());
  pigeon_list.write();

  // Extract token from connector
  let token = match &detail.pigeon.connector {
    Connector::Https(c) => c.token.clone(),
    Connector::Coap(c) => c.token.clone(),
  };

  Some((id, token))
}

pub async fn delete(pigeon_id: &str) -> Option<String> {
  let mut path = String::with_capacity(73);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);

  let _response = fetch_json("DELETE", &path, None).await?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  {
    let mut pigeons = pigeon_list.write();
    pigeons.remove(pigeon_id);
  }
  Some(pigeon_id.to_string())
}

pub async fn refresh_token(pigeon_id: &str) -> Option<String> {
  let mut path = String::with_capacity(87);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/token/refresh");

  let response = fetch_json("POST", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  let pigeon = serde_wasm_bindgen::from_value::<Pigeon>(json).ok()?;
  let token = match &pigeon.connector {
    Connector::Https(c) => c.token.clone(),
    Connector::Coap(c) => c.token.clone(),
  };

  // Update cache with new connector data
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.insert(pigeon_id.to_string(), pigeon);
  pigeon_list.write();

  Some(token)
}

pub async fn update_shadow(
  pigeon_id: &str,
  psur: &PigeonShadowUpdateRequest,
) -> Option<PigeonShadow> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/shadow");

  let json_string = serde_json::to_string(psur).ok()?;
  let body = serde_wasm_bindgen::to_value(&json_string).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  serde_wasm_bindgen::from_value::<PigeonShadow>(json).ok()
}

// PUT /pigeons/:id/telemetry-endpoint (task #18, landed in dovecote
// bc1373c), mirroring the other per-pigeon PUT routes above (e.g.
// /shadow). Unlike those, the route responds with the bare
// `Option<TelemetryEndpoint>` it just wrote (`Response::from_json(&endpoint)`
// in dovecote's lib.rs) rather than a full `Pigeon` — deserializing this as
// `Pigeon` would fail on every required field and silently collapse every
// call to `None`. Outer `Option` is request success; inner `Option` is "is
// an endpoint configured now" (`None` after clearing). No full `Pigeon`
// comes back, so there's nothing here to refresh `LocalSession` with —
// callers update their own `telemetry_endpoint` field from the return
// value instead (see `TelemetryEndpointModal`'s `on_saved` usage in
// views/pigeon.rs).
pub async fn update_telemetry_endpoint(
  pigeon_id: &str,
  petur: &PigeonTelemetryEndpointUpdateRequest,
) -> Option<Option<TelemetryEndpoint>> {
  let mut path = String::with_capacity(93);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry-endpoint");

  let body = serde_json::to_string(petur).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  serde_wasm_bindgen::from_value::<Option<TelemetryEndpoint>>(json).ok()
}
