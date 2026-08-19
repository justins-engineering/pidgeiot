use dioxus::logger::tracing::error;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCredentials, RequestInit, RequestMode, Response};

/// dovecote's router answers 401 in exactly one situation -- the Kratos
/// session cookie no longer resolves to a user -- and uses 403 for an
/// authenticated caller who simply isn't on a pigeon's ACL. So a 401 on
/// any route is an unambiguous "this tab's session is gone", and nothing
/// else is.
const UNAUTHORIZED: u16 = 401;

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
  // A dropped connection, DNS failure or CORS rejection fails the fetch
  // promise itself and leaves with `None` right here, never reaching the
  // status check below -- which is the whole reason that check can treat a
  // 401 as authoritative. Only a real HTTP response the browser accepted
  // gets a say in whether the session is still alive.
  let Ok(resp_value) = JsFuture::from(window.fetch_with_request(&request)).await else {
    crate::helpers::error_report::breadcrumb_api(method, path, None);
    return None;
  };
  let response = resp_value.dyn_into::<Response>().ok()?;

  // Second cross-cutting concern grafted onto the one funnel every API
  // call passes through (the 401 check below being the first): a
  // shape-only breadcrumb -- method, route template, status -- for the
  // error reporter's trail. Never bodies, never query params.
  crate::helpers::error_report::breadcrumb_api(method, path, Some(response.status()));

  // Every caller below eventually collapses a failed request to `None`,
  // which the views render as "nothing here" -- so without this an expired
  // session looks exactly like an account that owns no flocks, no pigeons
  // and no alerts. Catching it once at the single point every dovecote
  // request passes through is what keeps that from having to be remembered
  // at each of the call sites.
  if response.status() == UNAUTHORIZED {
    crate::helpers::session_lost();
  }

  Some(response)
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
/// `POST /pigeons/:id/shell` whose 400/403/409/502/504
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
