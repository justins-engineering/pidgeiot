use capsules::{CreateFlockPayload, Flock};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCredentials, RequestInit, RequestMode, Response};

pub async fn list() -> Option<HashMap<String, Flock>> {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks");

  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return None;
  };

  let Ok(_) = headers.append("Accept", "application/json") else {
    error!("Failed to set Accept headers!");
    return None;
  };

  let request_init = RequestInit::new();
  request_init.set_method("GET");
  request_init.set_mode(RequestMode::Cors);
  request_init.set_credentials(RequestCredentials::Include);
  request_init.set_headers(&headers);

  let request = Request::new_with_str_and_init(&location, &request_init);
  let Ok(request) = request else {
    error!("Failed to create flocks request!");
    return None;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read flocks request!");
    return None;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let json = JsFuture::from(response.json().unwrap_throw()).await;
  let Ok(json) = json else {
    error!("Failed to parse flocks response json!");
    return None;
  };

  match serde_wasm_bindgen::from_value::<Vec<Flock>>(json) {
    Ok(flocks_array) => {
      // Manually convert the Vec into the HashMap your UI state needs
      let flocks_map = flocks_array
        .into_iter()
        .map(|flock| (flock.id.clone(), flock))
        .collect();

      Some(flocks_map)
    }
    Err(e) => {
      error!("Deserialization Error: {e}");
      None
    }
  }
}

pub async fn create(flock: &CreateFlockPayload) {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks");

  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return;
  };

  let Ok(_) = headers.append("Accept", "application/json") else {
    error!("Failed to set Accept headers!");
    return;
  };

  let Ok(_) = headers.append("Content-Type", "application/json") else {
    error!("Failed to set Content-Type headers!");
    return;
  };

  // let body = flock.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
  let body = serde_json::to_string(flock);
  let Ok(body) = body else {
    error!("Failed to serialize Flock!");
    return;
  };

  let body = serde_wasm_bindgen::to_value(&body);
  let Ok(body) = body else {
    error!("Failed to convert Flock to JsValue!");
    return;
  };

  let request_init = RequestInit::new();
  request_init.set_method("POST");
  request_init.set_mode(RequestMode::Cors);
  request_init.set_credentials(RequestCredentials::Include);
  request_init.set_headers(&headers);
  request_init.set_body(&body);

  let request = Request::new_with_str_and_init(&location, &request_init);
  let Ok(request) = request else {
    error!("Failed to create flocks request!");
    return;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read flocks request!");
    return;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let json = JsFuture::from(response.json().unwrap_throw()).await;
  let Ok(json) = json else {
    error!("Failed to parse flocks response json!");
    return;
  };

  let mut flock_list = consume_context::<crate::LocalSession>().flocks;

  match serde_wasm_bindgen::from_value::<Flock>(json) {
    Ok(flock) => {
      debug!("{flock:?}");
      flock_list.insert(flock.id.to_string(), flock);
      flock_list.write();
    }
    Err(e) => {
      error!("{e}");
    }
  };
}

pub async fn update(flock_id: i64, flock: &Flock) {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks/");
  location.push_str(&flock_id.to_string());

  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return;
  };

  let Ok(_) = headers.append("Accept", "application/json") else {
    error!("Failed to set Accept headers!");
    return;
  };

  let Ok(_) = headers.append("Content-Type", "application/json") else {
    error!("Failed to set Content-Type headers!");
    return;
  };

  // let body = flock.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
  let body = serde_json::to_string(flock);
  let Ok(body) = body else {
    error!("Failed to serialize Flock!");
    return;
  };

  let body = serde_wasm_bindgen::to_value(&body);
  let Ok(body) = body else {
    error!("Failed to convert Flock to JsValue!");
    return;
  };

  let request_init = RequestInit::new();
  request_init.set_method("PUT");
  request_init.set_mode(RequestMode::Cors);
  request_init.set_credentials(RequestCredentials::Include);
  request_init.set_headers(&headers);
  request_init.set_body(&body);

  let request = Request::new_with_str_and_init(&location, &request_init);
  let Ok(request) = request else {
    error!("Failed to create flocks request!");
    return;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read flocks request!");
    return;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let json = JsFuture::from(response.json().unwrap_throw()).await;
  let Ok(json) = json else {
    error!("Failed to parse flocks response json!");
    return;
  };

  let mut flock_list = consume_context::<crate::LocalSession>().flocks;

  match serde_wasm_bindgen::from_value::<Flock>(json) {
    Ok(flock) => {
      debug!("{flock:?}");
      flock_list.insert(flock.id.clone(), flock);
      flock_list.write();
    }
    Err(e) => {
      error!("{e}");
    }
  };
}

pub async fn delete(flock_id: String) {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks/");
  location.push_str(&flock_id.to_string());

  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return;
  };

  let Ok(_) = headers.append("Accept", "application/json") else {
    error!("Failed to set Accept headers!");
    return;
  };

  let request_init = RequestInit::new();
  request_init.set_method("DELETE");
  request_init.set_mode(RequestMode::Cors);
  request_init.set_credentials(RequestCredentials::Include);
  request_init.set_headers(&headers);

  let request = Request::new_with_str_and_init(&location, &request_init);
  let Ok(request) = request else {
    error!("Failed to create flocks request!");
    return;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read flocks request!");
    return;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let mut flock_list = consume_context::<crate::LocalSession>().flocks;

  match response.status() {
    200 | 204 => {
      flock_list.remove(&flock_id);
      flock_list.write();
    }
    e => {
      error!("Failed to DELETE Pigeon! Response Status: {e}");
    }
  };
}
