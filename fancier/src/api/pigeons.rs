use capsules::Pigeon;
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCredentials, RequestInit, RequestMode, Response};

//  request_init.set_credentials(RequestCredentials::SameOrigin);

pub async fn list(flock_id: String) -> Option<HashMap<String, Pigeon>> {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks/");
  location.push_str(&flock_id.to_string());
  location.push_str("/pigeons");

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
    error!("Failed to create pigeons request!");
    return None;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read pigeons request!");
    return None;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let json = JsFuture::from(response.json().unwrap_throw()).await;
  let Ok(json) = json else {
    error!("Failed to parse pigeons response json!");
    return None;
  };
  // let mut list = consume_context::<crate::LocalSession>().pigeons;

  match serde_wasm_bindgen::from_value::<Vec<Pigeon>>(json) {
    Ok(resp) => {
      let mut pig_map = HashMap::<String, Pigeon>::new();
      for pig in resp {
        pig_map.insert(pig.id.to_string(), pig);
      }
      Some(pig_map)
    }
    Err(e) => {
      error!("{e}");
      None
    }
  }
}

pub async fn get(flock_id: String, pigeon_id: String) {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks/");
  location.push_str(&flock_id.to_string());
  location.push_str("/pigeons/");
  location.push_str(&pigeon_id.to_string());

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

  let request_init = RequestInit::new();
  request_init.set_method("GET");
  request_init.set_mode(RequestMode::Cors);
  request_init.set_credentials(RequestCredentials::Include);
  request_init.set_headers(&headers);

  let request = Request::new_with_str_and_init(&location, &request_init);
  let Ok(request) = request else {
    error!("Failed to create pigeons request!");
    return;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read pigeons request!");
    return;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let json = JsFuture::from(response.json().unwrap_throw()).await;
  let Ok(json) = json else {
    error!("Failed to parse pigeons response json!");
    return;
  };
}

pub async fn create(flock_id: String, pigeon: &Pigeon) {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks/");
  location.push_str(&flock_id.to_string());
  location.push_str("/pigeons");

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

  // let body = pigeon.serialize(&serde_wasm_bindgen::Serializer::json_compatible());
  let body = serde_json::to_string(pigeon);
  let Ok(body) = body else {
    error!("Failed to serialize Pigeon!");
    return;
  };

  let body = serde_wasm_bindgen::to_value(&body);
  let Ok(body) = body else {
    error!("Failed to convert Pigeon to JsValue!");
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
    error!("Failed to create pigeons request!");
    return;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read pigeons request!");
    return;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let json = JsFuture::from(response.json().unwrap_throw()).await;
  let Ok(json) = json else {
    error!("Failed to parse pigeons response json!");
    return;
  };

  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;

  match serde_wasm_bindgen::from_value::<Pigeon>(json) {
    Ok(pigeon) => {
      debug!("{pigeon:?}");
      pigeon_list.insert(pigeon.id.to_string(), pigeon);
      pigeon_list.write();
    }
    Err(e) => {
      error!("{e}");
    }
  };
}

pub async fn delete(flock_id: String, pigeon_id: String) {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str("/flocks/");
  location.push_str(&flock_id.to_string());
  location.push_str("/pigeons/");
  location.push_str(&pigeon_id.to_string());

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
    error!("Failed to create pigeons request!");
    return;
  };

  let resp_value = JsFuture::from(
    web_sys::window()
      .unwrap_throw()
      .fetch_with_request(&request),
  )
  .await;
  let Ok(resp_value) = resp_value else {
    error!("Failed to read pigeons request!");
    return;
  };

  assert!(resp_value.is_instance_of::<Response>());
  let response: Response = resp_value.dyn_into().unwrap();

  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;

  match response.status() {
    200 | 204 => {
      pigeon_list.remove(&pigeon_id);
      pigeon_list.write();
    }
    e => {
      error!("Failed to DELETE Pigeon! Response Status: {e}");
    }
  };
}
