use crate::api::fetch_json;
use capsules::{Pigeon, PigeonCreateRequest, PigeonDetail, PigeonUpdateRequest};
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

pub async fn create(pcrec: &PigeonCreateRequest) -> Option<String> {
  let body = serde_json::to_string(pcrec).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", "/flock/pigeons", Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  let pcres = serde_wasm_bindgen::from_value::<PigeonDetail>(json).ok()?;
  let pigeon = pcres.pigeon;
  let id = pigeon.id.clone();
  pigeon_list.insert(id.clone(), pigeon);
  pigeon_list.write();

  let mut flock_list = consume_context::<crate::LocalSession>().flocks;
  {
    let mut flocks = flock_list.write();
    if let Some(flock) = flocks.get_mut(&pcrec.flock_id) {
      flock.pigeon_ids.push(id.clone());
    }
  }

  Some(id)
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
