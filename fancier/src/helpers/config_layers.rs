//! The pure parts of the dashboard's config editors: reading a JSON file into an editor, and the
//! two layers a shadow's `target_config` is merged from (`capsules::flock_config`). Pure and
//! synchronous so they test off a wasm target.

use capsules::{
  FLOCK_FIRMWARE_REPUSH_KEY, JsonString, KeySource, effective_text, key_sources, parse_layer,
  validate_flock_layer,
};
use serde_json::{Map, Value};

/// The default `CONFIG_PIGEON_SHADOW_CONFIG_MAX`: a device built with it refuses a bigger merged
/// config, which the platform cannot see, so the editors warn past it.
pub const DEVICE_DEFAULT_CONFIG_MAX: usize = 320;

/// Client-side sanity cap on an uploaded config file, so a stray large file is a friendly error
/// rather than a textarea full of it; the servers' own caps are the authority.
pub const MAX_CONFIG_UPLOAD_BYTES: u64 = 64 * 1024;

/// Parses an uploaded config file and returns it pretty-printed for the editor. Only a JSON
/// object is accepted: anything else would only come back as a less specific 400.
pub fn parse_config_upload(text: &str) -> Result<String, String> {
  match serde_json::from_str::<Value>(text) {
    Ok(value @ Value::Object(_)) => {
      serde_json::to_string_pretty(&value).map_err(|err| format!("Invalid JSON: {err}"))
    }
    Ok(_) => Err(
      "The file must contain a JSON object (e.g. {\"key\": \"value\"}), not an array or a bare value."
        .to_string(),
    ),
    Err(err) => Err(format!("Invalid JSON: {err}")),
  }
}

/// The size the servers hold a config to: its compact serialization, whatever whitespace the
/// editor holds. `None` while the text is not valid JSON.
pub fn compact_config_bytes(raw: &str) -> Option<usize> {
  let value = serde_json::from_str::<Value>(raw).ok()?;
  serde_json::to_string(&value)
    .ok()
    .map(|compact| compact.len())
}

/// One key of a pigeon's config, for the shadow card's per-key table.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRow {
  pub key: String,
  /// The value the device is sent, or for a removed key the flock value it removes.
  pub value: String,
  pub source: KeySource,
}

fn object_of(json: &JsonString) -> Map<String, Value> {
  parse_layer(&json.to_string()).unwrap_or_default()
}

/// Every key either layer names, sorted, with where its value comes from.
pub fn layer_rows(pigeon: &JsonString, flock: &JsonString) -> Vec<LayerRow> {
  let pigeon = object_of(pigeon);
  let flock = object_of(flock);
  key_sources(&flock, &pigeon)
    .into_iter()
    .map(|(key, source)| {
      let value = match source {
        KeySource::Flock | KeySource::Removed => flock.get(&key),
        KeySource::Pigeon | KeySource::Override => pigeon.get(&key),
      };
      LayerRow {
        value: value.map(Value::to_string).unwrap_or_default(),
        key,
        source,
      }
    })
    .collect()
}

/// The words the dashboard shows for a key's source.
pub fn source_label(source: KeySource) -> &'static str {
  match source {
    KeySource::Flock => "from flock",
    KeySource::Pigeon => "this pigeon",
    KeySource::Override => "this pigeon, overrides flock",
    KeySource::Removed => "removed for this pigeon",
  }
}

/// Whether the shadow card has anything to say beyond the merged config: a flock layer, or a
/// removal in the pigeon's own.
pub fn has_layers(pigeon: &JsonString, flock: &JsonString) -> bool {
  !object_of(flock).is_empty() || object_of(pigeon).values().any(Value::is_null)
}

/// This pigeon's layer with `key` deleted, so it takes the flock's value again.
pub fn without_key(pigeon: &JsonString, key: &str) -> Result<Value, String> {
  let Some(mut layer) = parse_layer(&pigeon.to_string()) else {
    return Err("This pigeon's config isn't a JSON object.".to_string());
  };
  layer.remove(key);
  Ok(Value::Object(layer))
}

/// What the device would be sent were the editor's text saved as this pigeon's layer, compact.
pub fn preview(flock: &JsonString, editor_text: &str) -> Result<String, String> {
  match serde_json::from_str::<Value>(editor_text) {
    Ok(Value::Object(pigeon)) => Ok(effective_text(&object_of(flock), &pigeon)),
    Ok(_) => Err("The config must be a JSON object.".to_string()),
    Err(err) => Err(format!("Invalid JSON: {err}")),
  }
}

/// The flock editor's text as the flock write will take it, or the reason the write would refuse
/// it; the server's checks stay the authority.
pub fn check_flock_text(text: &str, adopt: &[String]) -> Result<Value, String> {
  let value = serde_json::from_str::<Value>(text).map_err(|err| format!("Invalid JSON: {err}"))?;
  validate_flock_layer(&value, adopt).map_err(|rejection| rejection.message().to_string())?;
  Ok(value)
}

/// The top-level keys of the editor's text, sorted; empty while it is not a JSON object.
pub fn layer_keys(text: &str) -> Vec<String> {
  match serde_json::from_str::<Value>(text) {
    Ok(Value::Object(layer)) => layer.keys().cloned().collect(),
    _ => Vec::new(),
  }
}

/// Whether a flock layer names a firmware image, the only case a flock-wide re-push reopens.
pub fn flock_has_firmware(layer: &JsonString) -> bool {
  object_of(layer)
    .get("firmware")
    .is_some_and(|firmware| !firmware.is_null())
}

/// The flock layer with its own re-push counter advanced and nothing else changed, so every
/// member that takes the flock's firmware sees a new `target_version`, whatever counter it holds
/// in its own layer.
pub fn repush_flock_layer(layer: &JsonString) -> Result<Value, String> {
  if !flock_has_firmware(layer) {
    return Err("This flock's config names no firmware to re-push.".to_string());
  }
  let mut layer = object_of(layer);
  let next = match layer.get(FLOCK_FIRMWARE_REPUSH_KEY).and_then(Value::as_u64) {
    Some(count) => count.checked_add(1).unwrap_or(0),
    None => 1,
  };
  layer.insert(FLOCK_FIRMWARE_REPUSH_KEY.to_string(), Value::from(next));
  Ok(Value::Object(layer))
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn config(raw: &str) -> JsonString {
    JsonString::new(raw.to_string()).expect("test config must be valid JSON")
  }

  #[test]
  fn the_byte_count_is_compact_utf8_whatever_the_editor_holds() {
    assert_eq!(
      compact_config_bytes("{\n  \"log\": 3,\n  \"on\": true\n}"),
      Some(19)
    );
    assert_eq!(compact_config_bytes("{\"s\":\"é\"}"), Some(10));
    assert_eq!(compact_config_bytes("{not json"), None);
  }

  #[test]
  fn an_upload_must_be_an_object_and_comes_back_pretty() {
    // serde_json's `Value::Object` is a `BTreeMap` in this workspace (no `preserve_order`), so
    // keys come back alphabetized whatever the file's order.
    assert_eq!(
      parse_config_upload(r#"{"telemetry_interval":60,"logging":"info"}"#),
      Ok("{\n  \"logging\": \"info\",\n  \"telemetry_interval\": 60\n}".to_string())
    );
    assert_eq!(parse_config_upload("{}"), Ok("{}".to_string()));
    for text in ["[1,2,3]", "42", "\"hello\""] {
      assert!(
        parse_config_upload(text)
          .unwrap_err()
          .contains("must contain a JSON object")
      );
    }
    assert!(
      parse_config_upload("{not valid json")
        .unwrap_err()
        .starts_with("Invalid JSON:")
    );
  }

  #[test]
  fn every_key_is_labelled_with_the_value_the_device_gets() {
    let pigeon = config(r#"{"b":20,"c":null,"d":4}"#);
    let flock = config(r#"{"a":1,"b":2,"c":3}"#);
    let rows = layer_rows(&pigeon, &flock);
    let found: Vec<(&str, &str, KeySource)> = rows
      .iter()
      .map(|row| (row.key.as_str(), row.value.as_str(), row.source))
      .collect();
    assert_eq!(
      found,
      vec![
        ("a", "1", KeySource::Flock),
        ("b", "20", KeySource::Override),
        ("c", "3", KeySource::Removed),
        ("d", "4", KeySource::Pigeon),
      ]
    );
    assert_eq!(
      source_label(KeySource::Override),
      "this pigeon, overrides flock"
    );
  }

  #[test]
  fn a_pigeon_with_no_flock_and_no_removal_has_no_layers_to_show() {
    assert!(!has_layers(&config(r#"{"a":1}"#), &config("{}")));
    assert!(has_layers(&config(r#"{"a":null}"#), &config("{}")));
    assert!(has_layers(&config("{}"), &config(r#"{"a":1}"#)));
  }

  #[test]
  fn using_the_flocks_value_deletes_only_that_key() {
    let pigeon = config(r#"{"stop_id":"414","telemetry_interval":10}"#);
    assert_eq!(
      without_key(&pigeon, "telemetry_interval"),
      Ok(json!({"stop_id": "414"}))
    );
    assert!(without_key(&config("[1]"), "a").is_err());
  }

  #[test]
  fn the_preview_is_what_the_device_will_be_sent() {
    let flock = config(r#"{"telemetry_interval":60,"log":"info"}"#);
    assert_eq!(
      preview(&flock, "{\n  \"stop_id\": \"1\",\n  \"log\": null\n}"),
      Ok(r#"{"stop_id":"1","telemetry_interval":60}"#.to_string())
    );
    assert!(preview(&flock, "[1]").is_err());
    assert!(preview(&flock, "{nope").is_err());
  }

  #[test]
  fn the_flock_editor_refuses_what_the_write_refuses() {
    assert!(check_flock_text(r#"{"telemetry_interval":60}"#, &[]).is_ok());
    assert!(
      check_flock_text(r#"{"reboot":true}"#, &[])
        .unwrap_err()
        .contains("reboot")
    );
    assert!(
      check_flock_text(r#"{"a":null}"#, &[])
        .unwrap_err()
        .contains("null")
    );
    assert!(check_flock_text("[1]", &[]).is_err());
    assert!(check_flock_text(r#"{"a":1}"#, &["b".to_string()]).is_err());
  }

  #[test]
  fn the_adopt_picker_offers_the_layers_keys() {
    assert_eq!(layer_keys(r#"{"b":1,"a":2}"#), vec!["a", "b"]);
    assert!(layer_keys("[1]").is_empty());
  }

  #[test]
  fn a_flock_re_push_advances_its_own_counter_only() {
    let layer = config(r#"{"firmware":{"version":"1","size":1,"sha256":"a"},"log":"info"}"#);
    let once = repush_flock_layer(&layer).unwrap();
    assert_eq!(once[FLOCK_FIRMWARE_REPUSH_KEY], json!(1));
    assert_eq!(once["log"], json!("info"));
    assert!(
      once.get("firmware_repush").is_none(),
      "the pigeon layer's key stays out"
    );
    let twice = repush_flock_layer(&config(&once.to_string())).unwrap();
    assert_eq!(twice[FLOCK_FIRMWARE_REPUSH_KEY], json!(2));
    assert!(repush_flock_layer(&config(r#"{"log":"info"}"#)).is_err());
  }
}
