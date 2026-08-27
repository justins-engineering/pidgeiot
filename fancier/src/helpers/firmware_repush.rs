use capsules::JsonString;
use serde_json::Value;

/// Top-level `target_config` key the re-push action advances.
///
/// It sits outside the `firmware` object deliberately. A device decodes
/// `firmware` into a fixed struct, and while Zephyr's `json_obj_parse`
/// skips fields it holds no descriptor for, a decoder that ever validates
/// its firmware target strictly would meet an unexpected key inside the
/// very thing it is about to flash. An unknown key at the *top* level is
/// the shape every shipped decoder already ignores, since that is how one
/// app's keys pass harmlessly through another app's build.
///
/// Kept short for the same reason it is an integer: the device library
/// caps one decoded config at `CONFIG_PIGEON_SHADOW_CONFIG_MAX` bytes
/// (320 by default) and a truncated config fails to parse rather than
/// degrading, so anything this dashboard adds unasked has to be tiny.
pub const REPUSH_KEY: &str = "firmware_repush";

/// Whether this `target_config` names a firmware image, which is the only
/// case where a re-push has anything to reopen.
pub fn has_firmware_target(config: &JsonString) -> bool {
  serde_json::from_str::<Value>(&config.to_string())
    .ok()
    .and_then(|value| value.get("firmware").cloned())
    .is_some_and(|firmware| !firmware.is_null())
}

/// Returns `target_config` with the re-push counter advanced, leaving every
/// other key -- the `firmware` object above all -- byte for byte as it was.
///
/// The platform advances a shadow's `target_version` when `target_config`
/// *changes*, not on every write (`increment_pigeon_target_version`, the
/// SQLite trigger in dovecote's Durable Object), so sending an identical
/// config back leaves a device looking at the same version it already gave
/// up on. Devices bound their FOTA retries to that version rather than to
/// the firmware version string, precisely so an operator can say "try
/// again" without republishing unchanged bytes under a new label. This
/// counter is the smallest thing that says it: the config differs, so the
/// version moves, while what the device actually applies does not.
///
/// Pure and synchronous so it can be tested off a wasm target, same
/// rationale as `merge_firmware_target` in `components/firmware_modal.rs`.
pub fn repush_target_config(config: &JsonString) -> Result<Value, String> {
  let mut value: Value = serde_json::from_str(&config.to_string())
    .map_err(|err| format!("Shadow target_config isn't valid JSON, can't re-push: {err}"))?;
  let obj = value
    .as_object_mut()
    .ok_or_else(|| "Shadow target_config isn't a JSON object.".to_string())?;

  let names_firmware = obj.get("firmware").is_some_and(|f| !f.is_null());
  if !names_firmware {
    return Err("This pigeon's shadow has no firmware target to re-push.".to_string());
  }

  let next = match obj.get(REPUSH_KEY).and_then(Value::as_u64) {
    // Wrap instead of saturating: only the change is meaningful, and a
    // counter pinned at its maximum would be a re-push that reopens
    // nothing. Anything that isn't a counter (a hand-edited string, a
    // float) restarts the count, which also changes the config.
    Some(count) => count.checked_add(1).unwrap_or(0),
    None => 1,
  };
  obj.insert(REPUSH_KEY.to_string(), Value::from(next));

  Ok(value)
}

#[cfg(test)]
mod tests {
  use super::{REPUSH_KEY, has_firmware_target, repush_target_config};
  use capsules::JsonString;
  use serde_json::{Value, json};

  fn config(raw: &str) -> JsonString {
    JsonString::new(raw.to_string()).expect("test config must be valid JSON")
  }

  fn firmware_text(value: &Value) -> String {
    serde_json::to_string(value.get("firmware").expect("firmware key must survive"))
      .expect("firmware value must serialize")
  }

  const WITH_FIRMWARE: &str = r#"{"firmware":{"version":"0.1.0+0","size":393802,"sha256":"aa"},"telemetry_interval":60,"log":true}"#;

  #[test]
  fn counter_starts_at_one_and_leaves_everything_else_alone() {
    let before = config(WITH_FIRMWARE);
    let after = repush_target_config(&before).expect("a firmware target must be re-pushable");

    assert_eq!(after.get(REPUSH_KEY), Some(&json!(1)));
    assert_eq!(
      firmware_text(&after),
      firmware_text(&serde_json::from_str::<Value>(WITH_FIRMWARE).unwrap()),
      "the firmware target must be re-sent unchanged"
    );
    assert_eq!(after.get("telemetry_interval"), Some(&json!(60)));
    assert_eq!(after.get("log"), Some(&json!(true)));
  }

  #[test]
  fn counter_advances_on_every_re_push() {
    let mut current = config(WITH_FIRMWARE);
    for expected in 1..=3u64 {
      let next = repush_target_config(&current).expect("re-push must stay available");
      assert_eq!(next.get(REPUSH_KEY), Some(&json!(expected)));
      current = config(&serde_json::to_string(&next).unwrap());
    }
  }

  #[test]
  fn a_counter_at_its_maximum_still_changes() {
    let raw = format!(r#"{{"firmware":{{"size":1}},"{REPUSH_KEY}":{}}}"#, u64::MAX);
    let after = repush_target_config(&config(&raw)).expect("re-push must stay available");
    assert_eq!(after.get(REPUSH_KEY), Some(&json!(0)));
  }

  #[test]
  fn a_non_counter_value_restarts_the_count() {
    let raw = format!(r#"{{"firmware":{{"size":1}},"{REPUSH_KEY}":"x"}}"#);
    let after = repush_target_config(&config(&raw)).expect("re-push must stay available");
    assert_eq!(after.get(REPUSH_KEY), Some(&json!(1)));
  }

  #[test]
  fn a_shadow_without_a_firmware_target_is_refused() {
    let no_firmware = config(r#"{"telemetry_interval":60}"#);
    assert!(repush_target_config(&no_firmware).is_err());
    assert!(repush_target_config(&config(r#"{"firmware":null}"#)).is_err());
    assert!(repush_target_config(&config("[]")).is_err());
  }

  #[test]
  fn presence_check_matches_what_the_transform_accepts() {
    let no_firmware = config(r#"{"telemetry_interval":60}"#);
    assert!(has_firmware_target(&config(WITH_FIRMWARE)));
    assert!(!has_firmware_target(&no_firmware));
    assert!(!has_firmware_target(&config(r#"{"firmware":null}"#)));
    assert!(!has_firmware_target(&config("[]")));
  }
}
