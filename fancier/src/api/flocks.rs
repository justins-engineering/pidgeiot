use crate::api::fetch_json;
use capsules::{Flock, FlockCreateRequest, FlockUpdateRequest};
use dioxus::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;
use wasm_bindgen_futures::JsFuture;

pub async fn list() -> Option<()> {
  let response = fetch_json("GET", "/flocks", None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let flocks_array = serde_wasm_bindgen::from_value::<Vec<Flock>>(json).ok()?;
  let flocks_map: HashMap<Uuid, Flock> = flocks_array
    .into_iter()
    .map(|flock| (flock.id, flock))
    .collect();

  let mut flock_list = consume_context::<crate::LocalSession>().flocks;
  *flock_list.write() = flocks_map;
  Some(())
}

pub async fn create(flock: &FlockCreateRequest) -> Option<Uuid> {
  let body = serde_json::to_string(flock).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", "/flocks", Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut flock_list = consume_context::<crate::LocalSession>().flocks;
  let flock = serde_wasm_bindgen::from_value::<Flock>(json).ok()?;
  let id: Uuid = flock.id;
  flock_list.insert(id, flock);
  flock_list.write();
  Some(id)
}

pub async fn update(flock_id: Uuid, flock: &FlockUpdateRequest) -> Option<Uuid> {
  let mut path = String::with_capacity(72);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());

  let body = serde_json::to_string(flock).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut flock_list = consume_context::<crate::LocalSession>().flocks;
  let flock = serde_wasm_bindgen::from_value::<Flock>(json).ok()?;
  let id = flock.id;
  flock_list.insert(id, flock);
  flock_list.write();
  Some(id)
}

pub async fn delete(flock_id: Uuid) -> Option<Uuid> {
  let mut path = String::with_capacity(72);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());

  let _response = fetch_json("DELETE", &path, None).await?;
  let mut flock_list = consume_context::<crate::LocalSession>().flocks;
  flock_list.remove(&flock_id);
  flock_list.write();
  Some(flock_id)
}
