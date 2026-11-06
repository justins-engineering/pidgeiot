//! Flock-level config: one JSON object every pigeon in a flock inherits, its **flock layer**,
//! laid under each pigeon's own object, its **pigeon layer**, one top-level key at a time.
//!
//! The device never sees either layer. It sees the **effective config** in
//! `PigeonShadow::target_config`, built by [`effective_config`]: start from the flock layer; a
//! pigeon key replaces the flock key whole, and a pigeon key set to `null` removes it. Values are
//! never merged below the top level, so `firmware` (version, size and sha256 together) and arrays
//! stay atomic. No `null` ever reaches a device: Zephyr's JSON decoder fails the whole parse on a
//! type mismatch, so a `null` for a key the app decodes would break every key it carries.

use std::ops::Deref;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{JsonString, PigeonShadow, deserialize_unix_float_to_i64};

/// The largest flock layer, serialized compactly: the top of `CONFIG_PIGEON_SHADOW_CONFIG_MAX`'s
/// range, so no device build can take more.
pub const MAX_FLOCK_CONFIG_BYTES: usize = 4096;

/// Top-level keys a flock layer may not carry. `reboot` is a one-shot command a device honours
/// on any unconverged push, so in a flock layer it would reboot every member on every later
/// change to any member's config. `firmware_repush` is the pigeon layer's own re-push counter;
/// a flock copy would be overridden for good by the first pigeon that re-pushes, so a flock
/// re-pushes with [`FLOCK_FIRMWARE_REPUSH_KEY`] instead.
pub const FLOCK_REFUSED_KEYS: [&str; 2] = ["reboot", "firmware_repush"];

/// The flock layer's firmware re-push counter, which no pigeon layer is expected to carry, so it
/// changes every member's effective config whatever counter a pigeon holds of its own.
pub const FLOCK_FIRMWARE_REPUSH_KEY: &str = "flock_firmware_repush";

/// The effective config of one pigeon: `flock` with `pigeon` laid over it per top-level key,
/// with every `null` dropped. Keys come out sorted, so the serialized text is deterministic.
pub fn effective_config(
  flock: &Map<String, Value>,
  pigeon: &Map<String, Value>,
) -> Map<String, Value> {
  let mut merged = flock.clone();
  for (key, value) in pigeon {
    if value.is_null() {
      merged.remove(key);
    } else {
      merged.insert(key.clone(), value.clone());
    }
  }
  merged.retain(|_, value| !value.is_null());
  merged
}

/// [`effective_config`] serialized as the shadow stores it.
pub fn effective_text(flock: &Map<String, Value>, pigeon: &Map<String, Value>) -> String {
  Value::Object(effective_config(flock, pigeon)).to_string()
}

/// A stored layer's text as an object; `None` when it is not a JSON object.
pub fn parse_layer(text: &str) -> Option<Map<String, Value>> {
  match serde_json::from_str::<Value>(text) {
    Ok(Value::Object(map)) => Some(map),
    _ => None,
  }
}

/// Whether the effective `firmware` comes from the flock: the flock layer names an image and the
/// pigeon layer has no `firmware` key at all (a `null` opts the pigeon out).
pub fn firmware_from_flock(flock: &Map<String, Value>, pigeon: &Map<String, Value>) -> bool {
  flock.get("firmware").is_some_and(|f| !f.is_null()) && !pigeon.contains_key("firmware")
}

/// Where one key of a pigeon's effective config comes from, for the dashboard's labels.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
  /// Inherited from the flock layer.
  Flock,
  /// Set by this pigeon; the flock layer has no such key.
  Pigeon,
  /// Set by this pigeon over a key the flock layer also carries.
  Override,
  /// Removed for this pigeon by a `null` in its layer; absent from the effective config.
  Removed,
}

/// Every key either layer names, sorted, with where its effective value comes from.
pub fn key_sources(
  flock: &Map<String, Value>,
  pigeon: &Map<String, Value>,
) -> Vec<(String, KeySource)> {
  let mut keys: Vec<&String> = flock.keys().chain(pigeon.keys()).collect();
  keys.sort();
  keys.dedup();
  keys
    .into_iter()
    .filter_map(|key| {
      let source = match (flock.get(key), pigeon.get(key)) {
        (_, Some(Value::Null)) => KeySource::Removed,
        (Some(_), Some(_)) => KeySource::Override,
        (None, Some(_)) => KeySource::Pigeon,
        (Some(Value::Null), None) | (None, None) => return None,
        (Some(_), None) => KeySource::Flock,
      };
      Some((key.clone(), source))
    })
    .collect()
}

/// Why a flock layer was refused at the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlockLayerRejection {
  NotAnObject,
  TooLarge,
  RefusedKey(&'static str),
  /// A flock key cannot be `null`: a flock removes a key by leaving it out.
  NullValue,
  /// An `adopt` key the new flock layer does not carry.
  AdoptUnknownKey,
}

impl FlockLayerRejection {
  /// The answer's text; never quotes the config.
  pub fn message(&self) -> &'static str {
    match self {
      Self::NotAnObject => "Bad Request: target_config must be a JSON object",
      Self::TooLarge => {
        "Payload Too Large: a flock's target_config must serialize to at most 4096 bytes"
      }
      Self::RefusedKey("reboot") => {
        "Bad Request: a flock's target_config cannot carry reboot; reboot pigeons one at a time"
      }
      Self::RefusedKey(_) => {
        "Bad Request: a flock's target_config cannot carry firmware_repush; use flock_firmware_repush"
      }
      Self::NullValue => "Bad Request: a flock key cannot be null; leave the key out instead",
      Self::AdoptUnknownKey => {
        "Bad Request: every adopt key must be a key of the new target_config"
      }
    }
  }

  /// 413 for the size cap, 400 for everything else.
  pub fn status(&self) -> u16 {
    match self {
      Self::TooLarge => 413,
      _ => 400,
    }
  }
}

/// Checks a flock layer as written: an object, no refused key, no `null`, at most
/// [`MAX_FLOCK_CONFIG_BYTES`], and every `adopt` key one it carries. Answers the layer and its
/// stored text.
pub fn validate_flock_layer(
  value: &Value,
  adopt: &[String],
) -> Result<(Map<String, Value>, String), FlockLayerRejection> {
  let Value::Object(layer) = value else {
    return Err(FlockLayerRejection::NotAnObject);
  };
  if let Some(key) = FLOCK_REFUSED_KEYS
    .iter()
    .find(|key| layer.contains_key(**key))
  {
    return Err(FlockLayerRejection::RefusedKey(key));
  }
  if layer.values().any(Value::is_null) {
    return Err(FlockLayerRejection::NullValue);
  }
  let text = value.to_string();
  if text.len() > MAX_FLOCK_CONFIG_BYTES {
    return Err(FlockLayerRejection::TooLarge);
  }
  if adopt.iter().any(|key| !layer.contains_key(key)) {
    return Err(FlockLayerRejection::AdoptUnknownKey);
  }
  Ok((layer.clone(), text))
}

/// Whether a flock layer may name an image built for `image_board`: only when the image is
/// tagged and every member is tagged with that same board. Fail-closed like the per-pigeon
/// check, so a flock that mixes boards keeps assigning firmware per pigeon.
pub fn flock_firmware_allowed(image_board: Option<&str>, member_boards: &[Option<&str>]) -> bool {
  image_board.is_some_and(|image| member_boards.iter().all(|board| *board == Some(image)))
}

/// A flock's layer as `GET`/`PUT /flocks/:flock_id/config` answer it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FlockConfig {
  pub flock_id: Uuid,
  pub target_config: JsonString,
  /// Moves by one on every write, including a write of the same layer (a resync).
  pub config_version: i32,
  /// Pigeons in the flock.
  pub members: i64,
  /// Members whose Postgres mirror says they took `config_version`; may lag about a minute.
  pub members_on_version: i64,
  /// On a write only: the apply messages queued, one per member.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub queued: Option<i64>,
}

/// Body of `PUT /flocks/:flock_id/config`. `{}` clears the layer. `adopt` names keys of the new
/// layer to delete from every member's pigeon layer, so those members take the flock's value.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FlockConfigUpdateRequest {
  pub target_config: Value,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub adopt: Vec<String>,
  /// The `config_version` the layer was edited from. A write over any other is refused, so an
  /// editor opened on a stale read never replaces a newer layer.
  pub config_version: i32,
}

/// The dashboard's view of a pigeon's shadow: the device's [`PigeonShadow`] plus both layers.
/// Never sent to a device, whose receive buffer is sized for one copy of the config.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonShadowView {
  #[serde(flatten)]
  pub shadow: PigeonShadow,
  /// This pigeon's own layer, `null` values kept: a `null` is how it removes a flock key.
  #[serde(default = "empty_object")]
  pub pigeon_config: JsonString,
  /// The flock layer this pigeon last took.
  #[serde(default = "empty_object")]
  pub flock_config: JsonString,
  #[serde(default)]
  pub flock_version: i32,
  /// Why the newest flock layer was not applied here, while it is not.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub flock_apply_error: Option<String>,
}

fn empty_object() -> JsonString {
  JsonString("{}".to_string())
}

impl Deref for PigeonShadowView {
  type Target = PigeonShadow;

  fn deref(&self) -> &PigeonShadow {
    &self.shadow
  }
}

/// A shadow with no layers of its own yet: its `target_config` is its pigeon layer.
impl From<PigeonShadow> for PigeonShadowView {
  fn from(shadow: PigeonShadow) -> Self {
    Self {
      pigeon_config: shadow.target_config.clone(),
      shadow,
      flock_config: empty_object(),
      flock_version: 0,
      flock_apply_error: None,
    }
  }
}

/// [`PigeonShadowView`] as the pigeon's Durable Object reads it: the shadow row beside its layer
/// row, whose columns are all `NULL` before the pigeon's first layer write.
#[derive(Deserialize, Debug)]
pub struct PigeonShadowViewRow {
  pub target_version: i32,
  pub current_version: i32,
  pub target_config: JsonString,
  pub current_config: JsonString,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub updated_at: i64,
  #[serde(default)]
  pub pigeon_config: Option<String>,
  #[serde(default)]
  pub flock_config: Option<String>,
  #[serde(default)]
  pub flock_version: Option<i32>,
  #[serde(default)]
  pub flock_apply_error: Option<String>,
}

impl From<PigeonShadowViewRow> for PigeonShadowView {
  fn from(row: PigeonShadowViewRow) -> Self {
    let pigeon_config = row
      .pigeon_config
      .and_then(|text| JsonString::new(text).ok())
      .unwrap_or_else(|| row.target_config.clone());
    let flock_config = row
      .flock_config
      .and_then(|text| JsonString::new(text).ok())
      .unwrap_or_else(empty_object);
    Self {
      shadow: PigeonShadow {
        target_version: row.target_version,
        current_version: row.current_version,
        target_config: row.target_config,
        current_config: row.current_config,
        updated_at: row.updated_at,
      },
      pigeon_config,
      flock_config,
      flock_version: row.flock_version.unwrap_or(0),
      flock_apply_error: row.flock_apply_error,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn map(value: Value) -> Map<String, Value> {
    match value {
      Value::Object(map) => map,
      _ => panic!("test layer is not an object"),
    }
  }

  #[test]
  fn the_flock_layer_alone_is_the_effective_config() {
    let flock = map(json!({"telemetry_interval": 60, "displays": [1, 2]}));
    assert_eq!(effective_config(&flock, &Map::new()), flock);
  }

  #[test]
  fn the_pigeon_layer_alone_is_the_effective_config() {
    let pigeon = map(json!({"stop_id": "414"}));
    assert_eq!(effective_config(&Map::new(), &pigeon), pigeon);
  }

  #[test]
  fn a_pigeon_key_replaces_the_flock_key_and_the_rest_is_inherited() {
    let flock = map(json!({"telemetry_interval": 60, "http_retry_count": 3}));
    let pigeon = map(json!({"telemetry_interval": 30, "stop_id": "73"}));
    assert_eq!(
      effective_config(&flock, &pigeon),
      map(json!({"telemetry_interval": 30, "http_retry_count": 3, "stop_id": "73"}))
    );
  }

  #[test]
  fn a_null_removes_a_flock_key_and_a_null_for_a_missing_key_removes_nothing() {
    let flock = map(json!({"telemetry_interval": 60, "log": "info"}));
    let pigeon = map(json!({"log": null, "absent": null}));
    assert_eq!(
      effective_config(&flock, &pigeon),
      map(json!({"telemetry_interval": 60}))
    );
  }

  #[test]
  fn objects_and_arrays_are_replaced_whole_never_mixed() {
    let flock = map(json!({
      "firmware": {"version": "1.0.0", "size": 10, "sha256": "aa"},
      "displays": [{"id": 1}, {"id": 2}],
    }));
    let pigeon = map(json!({
      "firmware": {"version": "0.9.0", "size": 9, "sha256": "bb"},
      "displays": [{"id": 3}],
    }));
    assert_eq!(effective_config(&flock, &pigeon), pigeon);
  }

  #[test]
  fn both_layers_empty_is_an_empty_object() {
    assert_eq!(effective_text(&Map::new(), &Map::new()), "{}");
  }

  #[test]
  fn no_null_ever_reaches_the_effective_config() {
    let flock = map(json!({"a": null, "b": 1}));
    let pigeon = map(json!({"c": null, "d": [null], "e": {"f": null}}));
    let merged = effective_config(&flock, &pigeon);
    assert!(merged.values().all(|value| !value.is_null()));
    assert_eq!(merged, map(json!({"b": 1, "d": [null], "e": {"f": null}})));
  }

  #[test]
  fn output_keys_are_sorted_and_the_text_is_stable() {
    let flock = map(json!({"zeta": 1, "alpha": 2}));
    let pigeon = map(json!({"mid": 3}));
    let text = effective_text(&flock, &pigeon);
    assert_eq!(text, r#"{"alpha":2,"mid":3,"zeta":1}"#);
    assert_eq!(text, effective_text(&flock, &pigeon));
  }

  #[test]
  fn a_pigeon_with_no_flock_layer_keeps_its_text_byte_for_byte() {
    let body = json!({"telemetry_interval": 60, "stop_id": "414", "displays": [1]});
    assert_eq!(
      effective_text(&Map::new(), &map(body.clone())),
      body.to_string()
    );
  }

  #[test]
  fn only_an_object_is_a_layer() {
    assert!(parse_layer("{}").is_some());
    for text in ["[]", "42", "\"x\"", "null", "{not json"] {
      assert_eq!(parse_layer(text), None, "{text}");
    }
  }

  #[test]
  fn validation_refuses_non_objects_refused_keys_nulls_and_the_cap() {
    for value in [json!([]), json!(1), json!("x"), Value::Null] {
      assert_eq!(
        validate_flock_layer(&value, &[]),
        Err(FlockLayerRejection::NotAnObject)
      );
    }
    assert_eq!(
      validate_flock_layer(&json!({"reboot": true}), &[]),
      Err(FlockLayerRejection::RefusedKey("reboot"))
    );
    assert_eq!(
      validate_flock_layer(&json!({"firmware_repush": 1}), &[]),
      Err(FlockLayerRejection::RefusedKey("firmware_repush"))
    );
    assert_eq!(
      validate_flock_layer(&json!({"log": null}), &[]),
      Err(FlockLayerRejection::NullValue)
    );
    assert_eq!(
      validate_flock_layer(&json!({"log": "info"}), &["stop_id".to_string()]),
      Err(FlockLayerRejection::AdoptUnknownKey)
    );
  }

  #[test]
  fn the_cap_counts_the_serialized_bytes() {
    // {"k":"<v>"} is 8 bytes of punctuation and key around the value.
    let fits = json!({"k": "x".repeat(MAX_FLOCK_CONFIG_BYTES - 8)});
    let (_, text) = validate_flock_layer(&fits, &[]).unwrap();
    assert_eq!(text.len(), MAX_FLOCK_CONFIG_BYTES);

    let over = json!({"k": "x".repeat(MAX_FLOCK_CONFIG_BYTES - 7)});
    let refused = validate_flock_layer(&over, &[]).unwrap_err();
    assert_eq!(refused, FlockLayerRejection::TooLarge);
    assert_eq!(refused.status(), 413);
  }

  #[test]
  fn a_valid_layer_answers_its_compact_text() {
    let value = json!({"telemetry_interval": 60, "stop_id": "1"});
    let (layer, text) = validate_flock_layer(&value, &["stop_id".to_string()]).unwrap();
    assert_eq!(text, r#"{"stop_id":"1","telemetry_interval":60}"#);
    assert_eq!(layer.len(), 2);
  }

  #[test]
  fn flock_firmware_needs_one_tagged_board_across_every_member() {
    let board = Some("circuitdojo_feather/nrf9160/ns");
    assert!(flock_firmware_allowed(board, &[board, board]));
    assert!(
      flock_firmware_allowed(board, &[]),
      "an empty flock checks each pigeon on arrival"
    );
    assert!(!flock_firmware_allowed(None, &[None]), "an untagged image");
    assert!(
      !flock_firmware_allowed(board, &[board, None]),
      "an untagged member"
    );
    assert!(
      !flock_firmware_allowed(board, &[board, Some("esp32c6_devkitc")]),
      "mixed boards"
    );
  }

  #[test]
  fn firmware_comes_from_the_flock_unless_the_pigeon_names_or_nulls_its_own() {
    let flock = map(json!({"firmware": {"version": "1"}}));
    assert!(firmware_from_flock(&flock, &Map::new()));
    assert!(!firmware_from_flock(
      &flock,
      &map(json!({"firmware": null}))
    ));
    assert!(!firmware_from_flock(
      &flock,
      &map(json!({"firmware": {"version": "0"}}))
    ));
    assert!(!firmware_from_flock(&Map::new(), &Map::new()));
  }

  #[test]
  fn every_key_is_labelled_with_its_source() {
    let flock = map(json!({"a": 1, "b": 2, "c": 3}));
    let pigeon = map(json!({"b": 20, "c": null, "d": 4, "e": null}));
    assert_eq!(
      key_sources(&flock, &pigeon),
      vec![
        ("a".to_string(), KeySource::Flock),
        ("b".to_string(), KeySource::Override),
        ("c".to_string(), KeySource::Removed),
        ("d".to_string(), KeySource::Pigeon),
        ("e".to_string(), KeySource::Removed),
      ]
    );
  }

  #[test]
  fn the_view_flattens_the_device_shadow_and_reads_back() {
    let view = PigeonShadowView {
      shadow: PigeonShadow {
        target_version: 3,
        current_version: 2,
        target_config: JsonString::new(r#"{"a":1,"b":2}"#.to_string()).unwrap(),
        current_config: JsonString::new("{}".to_string()).unwrap(),
        updated_at: 1_762_000_000,
      },
      pigeon_config: JsonString::new(r#"{"b":2,"c":null}"#.to_string()).unwrap(),
      flock_config: JsonString::new(r#"{"a":1,"c":3}"#.to_string()).unwrap(),
      flock_version: 7,
      flock_apply_error: None,
    };
    let text = serde_json::to_string(&view).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["target_version"], 3);
    assert_eq!(value["updated_at"], 1_762_000_000);
    assert!(value.get("flock_apply_error").is_none());
    assert_eq!(
      serde_json::from_str::<PigeonShadowView>(&text).unwrap(),
      view
    );

    // An older dovecote answers the bare device shadow; the layers default.
    let bare = serde_json::to_string(&view.shadow).unwrap();
    let read = serde_json::from_str::<PigeonShadowView>(&bare).unwrap();
    assert_eq!(read.shadow, view.shadow);
    assert_eq!(read.flock_version, 0);
    assert_eq!(read.target_version, 3, "derefs to the device shadow");
  }

  #[test]
  fn a_shadow_with_no_layer_row_is_its_own_pigeon_layer() {
    let row = PigeonShadowViewRow {
      target_version: 1,
      current_version: 1,
      target_config: JsonString::new(r#"{"stop_id":"414"}"#.to_string()).unwrap(),
      current_config: JsonString::new("{}".to_string()).unwrap(),
      updated_at: 1,
      pigeon_config: None,
      flock_config: None,
      flock_version: None,
      flock_apply_error: None,
    };
    let view = PigeonShadowView::from(row);
    assert_eq!(view.pigeon_config, view.shadow.target_config);
    assert_eq!(view.flock_config.to_string(), "{}");
    assert_eq!(view.flock_version, 0);
  }

  #[test]
  fn the_update_request_omits_an_empty_adopt_list() {
    let body = FlockConfigUpdateRequest {
      target_config: json!({}),
      adopt: Vec::new(),
      config_version: 3,
    };
    let text = r#"{"target_config":{},"config_version":3}"#;
    assert_eq!(serde_json::to_string(&body).unwrap(), text);
    let read: FlockConfigUpdateRequest = serde_json::from_str(text).unwrap();
    assert!(read.adopt.is_empty());
  }

  #[test]
  fn the_update_request_needs_the_version_it_was_edited_from() {
    assert!(serde_json::from_str::<FlockConfigUpdateRequest>(r#"{"target_config":{}}"#).is_err());
  }
}
