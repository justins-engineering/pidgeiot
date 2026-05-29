use dioxus::logger::tracing::error;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCredentials, RequestInit, RequestMode, Response};

pub async fn fetch_json(method: &str, path: &str, body: Option<&JsValue>) -> Option<Response> {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str(path);

  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return None;
  };
  headers.append("Accept", "application/json").ok()?;

  if body.is_some() {
    headers.append("Content-Type", "application/json").ok()?;
  }

  let request_init = RequestInit::new();
  request_init.set_method(method);
  request_init.set_mode(RequestMode::Cors);
  request_init.set_credentials(RequestCredentials::Include);
  request_init.set_headers(&headers);
  if let Some(b) = body {
    request_init.set_body(b);
  }

  let request = Request::new_with_str_and_init(&location, &request_init).ok()?;
  let window = web_sys::window()?;
  let resp_value = JsFuture::from(window.fetch_with_request(&request))
    .await
    .ok()?;
  let response = resp_value.dyn_into::<Response>().ok()?;

  if !response.ok() {
    error!("{method} {path} failed with status: {}", response.status());
    return None;
  }

  Some(response)
}
