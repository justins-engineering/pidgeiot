use dioxus::logger::tracing::error;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCredentials, RequestInit, RequestMode, Response};

/// Shared request dispatch for both `fetch_json` and `fetch_bytes` below --
/// method/mode/credentials/headers/body wiring and the JS `fetch()` round
/// trip are identical either way; only the headers (JSON vs. raw-bytes
/// `Content-Type`) and the body's `JsValue` representation differ between
/// callers, so those are built by each public wrapper and handed in here.
async fn dispatch(
  method: &str,
  path: &str,
  headers: Headers,
  body: Option<&JsValue>,
) -> Option<Response> {
  let mut location = String::with_capacity(128);
  location.push_str(crate::config::API_HOST);
  location.push_str(path);

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
  resp_value.dyn_into::<Response>().ok()
}

pub async fn fetch_json(method: &str, path: &str, body: Option<&JsValue>) -> Option<Response> {
  let response = fetch_json_any_status(method, path, body).await?;

  if !response.ok() {
    error!("{method} {path} failed with status: {}", response.status());
    return None;
  }

  Some(response)
}

/// Like `fetch_json`, but hands back the `Response` regardless of HTTP
/// status instead of collapsing every non-2xx to `None` -- for callers
/// that need to distinguish *which* error occurred (status code + body
/// text), not just "it failed". Most routes only care about success vs.
/// failure, which `fetch_json` already covers; this exists for routes like
/// `POST /pigeons/:id/shell` (task #34) whose 400/403/409/502/504
/// responses are each a distinct, actionable state the UI shows
/// differently rather than one generic error.
pub async fn fetch_json_any_status(
  method: &str,
  path: &str,
  body: Option<&JsValue>,
) -> Option<Response> {
  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return None;
  };
  headers.append("Accept", "application/json").ok()?;

  if body.is_some() {
    headers.append("Content-Type", "application/json").ok()?;
  }

  dispatch(method, path, headers, body).await
}

/// Like `fetch_json`, but for routes whose request body **is** raw bytes
/// rather than a JSON-encoded string -- dovecote's `POST
/// /flocks/:flock_id/firmware` (docs/api.md's "Firmware" section, same
/// convention as `POST /device/pigeons/:pigeon_id/logs`) reads the body via
/// `req.bytes()`, not `req.json()`, so sending `Content-Type:
/// application/json` here would mislabel the payload even though dovecote
/// doesn't currently check the header. The response is still JSON, so
/// `Accept: application/json` stays.
pub async fn fetch_bytes(method: &str, path: &str, body: &[u8]) -> Option<Response> {
  let Ok(headers) = Headers::new() else {
    error!("Failed to create fetch headers!");
    return None;
  };
  headers.append("Accept", "application/json").ok()?;
  headers
    .append("Content-Type", "application/octet-stream")
    .ok()?;

  let array = js_sys::Uint8Array::from(body);
  let response = dispatch(method, path, headers, Some(array.as_ref())).await?;

  if !response.ok() {
    error!("{method} {path} failed with status: {}", response.status());
    return None;
  }

  Some(response)
}
