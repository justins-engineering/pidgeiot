// dovecote's dashboard-state routes (docs/api.md's "Dashboard state"
// section) -- one opaque JSON document per scope key, owned by the signed-in
// account. Nothing here knows what a document holds; `helpers::graph_store`
// owns the only shape stored so far.
use crate::api::{fetch_json, fetch_json_any_status};
use capsules::DashboardStateEntry;
use dioxus::logger::tracing::error;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

/// dovecote answers 404 for a scope the account has never saved, which is
/// a different fact from a request that never arrived -- see [`StateRead`].
const NOT_FOUND: u16 = 404;

/// What a read of one scope found. The third case is why this isn't an
/// `Option`: a client may push its own copy up over a document the server
/// says does not exist, but never over one it simply failed to read.
pub enum StateRead {
  Found(DashboardStateEntry),
  /// The account has no document for this scope.
  Missing,
  /// The request failed, so the server's copy is unknown.
  Unavailable,
}

fn path_for(scope_key: &str) -> String {
  let mut path = String::with_capacity(17 + scope_key.len());
  path.push_str("/dashboard-state/");
  path.push_str(scope_key);
  path
}

/// `GET /dashboard-state/:scope_key`.
pub async fn get(scope_key: &str) -> StateRead {
  let path = path_for(scope_key);
  let Some(response) = fetch_json_any_status("GET", &path, None).await else {
    return StateRead::Unavailable;
  };

  if response.status() == NOT_FOUND {
    return StateRead::Missing;
  }
  if !response.ok() {
    error!("GET {path} failed with status: {}", response.status());
    return StateRead::Unavailable;
  }

  let Ok(promise) = response.json() else {
    return StateRead::Unavailable;
  };
  let Ok(json) = JsFuture::from(promise).await else {
    return StateRead::Unavailable;
  };
  match serde_wasm_bindgen::from_value::<DashboardStateEntry>(json) {
    Ok(entry) => StateRead::Found(entry),
    Err(e) => {
      error!("GET {path} returned an unreadable entry: {e}");
      StateRead::Unavailable
    }
  }
}

/// `PUT /dashboard-state/:scope_key` -- `value` is the JSON document
/// itself, sent as the whole body. The entry that comes back is the stored
/// one, and the only read guaranteed to reflect the write.
pub async fn put(scope_key: &str, value: &str) -> Option<DashboardStateEntry> {
  let body = JsValue::from_str(value);
  let response = fetch_json("PUT", &path_for(scope_key), Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}
