// Thin localStorage wrapper: a flat JSON blob per scoped key. Namespaced
// and versioned so a v2 schema change can migrate or ignore v1 entries
// instead of failing to deserialize them. What is kept here is either
// this browser's own business (a one-shot handoff) or a mirror of
// something the account owns server-side (helpers/graph_store.rs).
use serde::Serialize;
use serde::de::DeserializeOwned;

fn storage() -> Option<web_sys::Storage> {
  web_sys::window()?.local_storage().ok().flatten()
}

pub fn load<T: DeserializeOwned>(key: &str) -> Option<T> {
  let raw = storage()?.get_item(key).ok().flatten()?;
  serde_json::from_str(&raw).ok()
}

pub fn save<T: Serialize>(key: &str, value: &T) -> Option<()> {
  let raw = serde_json::to_string(value).ok()?;
  storage()?.set_item(key, &raw).ok()
}

/// For keys that are consumed rather than kept -- a one-shot handoff left
/// behind after being read would be picked up again by a later, unrelated
/// visit.
pub fn remove(key: &str) -> Option<()> {
  storage()?.remove_item(key).ok()
}
