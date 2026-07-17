// Thin localStorage wrapper for client-only persistence (task #19's graph
// definitions). v1 is a flat JSON blob per scoped key — deliberately not a
// capsules type or a server round-trip: server-side persistence of graph
// definitions is a later upgrade once there's a natural place to hang it on
// the Pigeon/Flock API (see components/graph_widget.rs's GraphDef doc
// comment). Namespaced and versioned so a v2 schema change can migrate or
// ignore v1 entries instead of failing to deserialize them.
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
