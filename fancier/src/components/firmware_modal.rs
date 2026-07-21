use crate::api;
use crate::components::{BOARD_DATALIST_ID, BoardDatalist};
use capsules::{
  FirmwareImage, FirmwareTarget, JsonString, PigeonShadow, PigeonShadowUpdateRequest,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;
use uuid::Uuid;

fn format_bytes(len: i64) -> String {
  let len = len.max(0) as u64;
  if len < 1024 {
    format!("{len} B")
  } else {
    format!("{:.1} KB", len as f64 / 1024.0)
  }
}

/// Pulls `target_config.firmware`/`current_config.firmware`
/// (`capsules::FirmwareTarget`, task #23) out of a shadow's `JsonString`
/// field for display -- `None` covers both "not valid JSON" (shouldn't
/// happen for a config this dashboard itself wrote) and "no `firmware` key
/// set yet" (the common case for any pigeon that predates FOTA or has
/// never been assigned an image). Pure and synchronous so it's
/// unit-testable without a wasm target, same rationale as
/// `parse_shadow_upload` in views/pigeon.rs (task #26).
fn extract_firmware_target(config: &JsonString) -> Option<FirmwareTarget> {
  let value: serde_json::Value = serde_json::from_str(&config.to_string()).ok()?;
  let firmware = value.get("firmware")?.clone();
  serde_json::from_value(firmware).ok()
}

/// Fail-closed board/geometry compatibility check (task #20, phase 1),
/// mirrored client-side from dovecote's own `check_firmware_board_compat`
/// (`objects/pigeons.rs`) -- both sides must be set AND equal; an unset
/// pigeon board, an unset/untagged image board, or an explicit mismatch
/// are all incompatible, not a free pass. This is a courtesy that avoids
/// the round trip for the common case and lets the Assign button explain
/// itself instead of failing with a bare 400 -- dovecote still enforces
/// the real check server-side regardless, so this being wrong in some edge
/// case (e.g. stale client state) can't itself cause an incorrect
/// assignment, only an avoidable failed request.
fn boards_compatible(pigeon_board: Option<&str>, image_board: Option<&str>) -> bool {
  match (pigeon_board, image_board) {
    (Some(p), Some(i)) => p == i,
    _ => false,
  }
}

/// Merges a `firmware` key into an existing `target_config` (task #23/#25B
/// -- "Firmware assignment reuses [the shadow PUT] route... Merge a
/// `firmware` key into `target_config`", docs/api.md's "Shadow" section).
/// dovecote's `PUT /pigeons/:id/shadow` replaces `target_config` wholesale
/// (see `objects/pigeons.rs::update_shadow`) rather than merging
/// server-side, so the merge has to happen here before sending the full
/// object back -- naively sending just `{"firmware": {...}}` would silently
/// wipe out any other keys (e.g. `telemetry_interval`) already targeted.
/// Errors if the existing `target_config` isn't a JSON object, which
/// shouldn't happen given `EditShadowModal`'s own object-only validation
/// (task #26) but is checked explicitly rather than assumed.
fn merge_firmware_target(
  config: &JsonString,
  target: &FirmwareTarget,
) -> Result<serde_json::Value, String> {
  let mut value: serde_json::Value = serde_json::from_str(&config.to_string()).map_err(|err| {
    format!("Existing shadow target_config isn't valid JSON, can't merge firmware: {err}")
  })?;
  let obj = value
    .as_object_mut()
    .ok_or_else(|| "Existing shadow target_config isn't a JSON object.".to_string())?;
  let firmware_value = serde_json::to_value(target)
    .map_err(|err| format!("Failed to serialize firmware target: {err}"))?;
  obj.insert("firmware".to_string(), firmware_value);
  Ok(value)
}

/// Owner-gated firmware upload + per-pigeon assignment (task #25 Part B).
/// Rendered conditionally by the caller rather than a native `<dialog>`,
/// per the reset-sensitive modal pattern in CLAUDE.md: the selected file,
/// computed hash, and version label must never linger across an
/// open/cancel/reopen cycle.
///
/// Upload targets the flock (`api::firmware::upload`, `POST
/// /flocks/:flock_id/firmware`) since firmware images are catalogued
/// per-flock, not per-pigeon (`capsules::FirmwareImage`'s doc comment) --
/// this pigeon's own "Upload Firmware" button is just a convenient place to
/// reach the flock's catalog from. Assignment goes through the existing
/// shadow `PUT` (`api::pigeons::update_shadow`) with a merged
/// `target_config.firmware` (task #24's frozen wire shape) -- there is no
/// separate "assign firmware" endpoint. `dovecote` never trusts a
/// client-supplied hash for the upload itself (`size`/`sha256` are always
/// server-computed), so the client-side `helpers::sha256_hex` result here
/// is a display/sanity-check value only, compared against the server's
/// response after upload.
#[component]
pub fn FirmwareModal(
  flock_id: Uuid,
  pigeon_id: String,
  pigeon_board: Option<String>,
  shadow: PigeonShadow,
  on_close: EventHandler<()>,
  on_assigned: EventHandler<PigeonShadow>,
) -> Element {
  let current_target = extract_firmware_target(&shadow.current_config);
  let target_target = extract_firmware_target(&shadow.target_config);
  let base_target_config = shadow.target_config.clone();

  let mut images: Signal<Option<Vec<FirmwareImage>>> = use_signal(|| None);
  let mut images_error = use_signal(|| false);

  use_resource(move || async move {
    match api::firmware::list(flock_id).await {
      Some(list) => images.set(Some(list)),
      None => images_error.set(true),
    }
  });

  let mut selected_bytes: Signal<Option<Vec<u8>>> = use_signal(|| None);
  let mut selected_name = use_signal(|| Option::<String>::None);
  let mut selected_hash = use_signal(|| Option::<String>::None);
  let mut is_hashing = use_signal(|| false);
  let mut file_error = use_signal(|| Option::<String>::None);
  let mut version_input = use_signal(String::new);
  let mut board_input = use_signal(String::new);
  let mut is_uploading = use_signal(|| false);
  let mut upload_error = use_signal(|| Option::<String>::None);
  let mut upload_result: Signal<Option<FirmwareImage>> = use_signal(|| None);

  let mut assigning_id: Signal<Option<Uuid>> = use_signal(|| None);
  let mut assign_error = use_signal(|| Option::<String>::None);

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "firmware_modal_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-2xl",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 { class: "text-lg font-bold", id: "firmware_modal_title", "Firmware" }

        div { class: "overflow-x-auto mt-3",
          table { class: "table table-sm",
            tbody {
              tr {
                th { "Current (device-reported)" }
                td { class: "font-mono text-sm",
                  match &current_target {
                    Some(t) => rsx! {
                      "{t.version}"
                    },
                    None => rsx! {
                      span { class: "text-base-content/50 italic", "none reported" }
                    },
                  }
                }
              }
              tr {
                th { "Target (assigned)" }
                td { class: "font-mono text-sm",
                  match &target_target {
                    Some(t) => rsx! {
                      "{t.version}"
                    },
                    None => rsx! {
                      span { class: "text-base-content/50 italic", "none assigned" }
                    },
                  }
                }
              }
              tr {
                th { "This pigeon's board" }
                td { class: "font-mono text-sm",
                  match pigeon_board.as_deref() {
                    Some(board) => rsx! {
                      "{board}"
                    },
                    None => rsx! {
                      span { class: "text-warning italic",
                        "untagged — set via the pigeon's Edit button before assigning firmware"
                      }
                    },
                  }
                }
              }
            }
          }
        }

        div { class: "divider text-xs", "Upload a new image" }

        fieldset { class: "fieldset flex flex-col gap-3",
          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Firmware file" }
            input {
              r#type: "file",
              class: "file-input file-input-bordered file-input-sm w-full",
              onchange: move |evt: Event<FormData>| {
                  async move {
                      file_error.set(None);
                      selected_bytes.set(None);
                      selected_name.set(None);
                      selected_hash.set(None);
                      upload_result.set(None);
                      upload_error.set(None);
                      let Some(file) = evt.files().into_iter().next() else {
                          return;
                      };
                      if file.size() > capsules::MAX_FIRMWARE_BYTES as u64 {
                          file_error
                              .set(
                                  Some(
                                      format!(
                                          "File is {} KB, which is over the {} KB limit.",
                                          file.size() / 1024,
                                          capsules::MAX_FIRMWARE_BYTES / 1024,
                                      ),
                                  ),
                              );
                          return;
                      }
                      let bytes = match file.read_bytes().await {
                          Ok(b) => b.to_vec(),
                          Err(err) => {
                              file_error.set(Some(format!("Failed to read file: {err}")));
                              return;
                          }
                      };
                      is_hashing.set(true);
                      let hash = crate::helpers::sha256_hex(&bytes).await;
                      is_hashing.set(false);
                      let Some(hash) = hash else {
                          file_error.set(Some("Failed to hash the file in-browser.".to_string()));
                          return;
                      };
                      selected_name.set(Some(file.name()));
                      selected_hash.set(Some(hash));
                      selected_bytes.set(Some(bytes));
                  }
              },
            }
            if let Some(err) = file_error.read().as_ref() {
              p { class: "text-error text-xs mt-1", "⚠️ {err}" }
            }
          }

          if let Some(name) = selected_name.read().as_ref() {
            div { class: "text-xs font-mono bg-base-200 rounded p-2 flex flex-col gap-1",
              div {
                "{name} — {selected_bytes.read().as_ref().map(|b| b.len()).unwrap_or(0)} bytes"
              }
              if is_hashing() {
                div { class: "flex items-center gap-2",
                  span { class: "loading loading-spinner loading-xs" }
                  "Hashing..."
                }
              } else if let Some(hash) = selected_hash.read().as_ref() {
                div { class: "break-all", "sha256: {hash}" }
              }
            }
          }

          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Version label" }
            input {
              class: "input input-bordered input-sm w-full text-sm font-mono",
              r#type: "text",
              placeholder: "e.g., 0.1.0+0",
              value: "{version_input}",
              oninput: move |e| version_input.set(e.value()),
            }
          }

          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Board (required)" }
            input {
              class: "input input-bordered input-sm w-full text-sm font-mono",
              r#type: "text",
              list: BOARD_DATALIST_ID,
              autocomplete: "off",
              placeholder: "e.g., circuitdojo_feather/nrf9160/ns",
              value: "{board_input}",
              oninput: move |e| board_input.set(e.value()),
            }
            p { class: "text-xs text-base-content/60 mt-1",
              "The exact CONFIG_BOARD_TARGET this image was built for — dovecote rejects the upload without it, and won't assign this image to a pigeon whose own board doesn't match exactly."
            }
          }

          if let Some(err) = upload_error.read().as_ref() {
            p { class: "text-error text-xs", "⚠️ {err}" }
          }

          if let Some(image) = upload_result.read().as_ref() {
            {
                let hash_matches = selected_hash.read().as_deref() == Some(image.sha256.as_str());
                rsx! {
                  p {
                    class: if hash_matches { "text-success text-xs" } else { "text-warning text-xs" },
                    if hash_matches {
                      "Uploaded — server sha256 matches the client-computed hash."
                    } else {
                      "Uploaded, but the server's sha256 didn't match the client-computed hash — re-check the file."
                    }
                  }
                }
            }
          }

          button {
            class: "btn btn-primary btn-sm self-start",
            disabled: selected_bytes.read().is_none() || version_input.read().trim().is_empty()
                || board_input.read().trim().is_empty() || is_uploading() || is_hashing(),
            onclick: move |_| {
                let bytes = selected_bytes.read().clone();
                let version = version_input.read().trim().to_string();
                let board = board_input.read().trim().to_string();
                async move {
                    let Some(bytes) = bytes else {
                        return;
                    };
                    is_uploading.set(true);
                    upload_error.set(None);
                    match api::firmware::upload(flock_id, &version, &board, &bytes).await {
                        Some(image) => {
                            is_uploading.set(false);
                            upload_result.set(Some(image.clone()));
                            if let Some(list) = images.write().as_mut() {
                                list.retain(|existing| existing.sha256 != image.sha256);
                                list.insert(0, image);
                            }
                        }
                        None => {
                            is_uploading.set(false);
                            upload_error
                                .set(
                                    Some("Failed to upload firmware. Please try again.".to_string()),
                                );
                        }
                    }
                }
            },
            if is_uploading() {
              span { class: "loading loading-spinner loading-xs" }
            } else {
              "Upload"
            }
          }
        }

        div { class: "divider text-xs", "Assign to this pigeon" }

        if let Some(err) = assign_error.read().as_ref() {
          p { class: "text-error text-xs mb-2", "⚠️ {err}" }
        }

        match images.read().clone() {
          None if !images_error() => rsx! {
            div { class: "loading loading-spinner text-primary loading-sm" }
          },
          None => rsx! {
            p { class: "text-error text-xs", "Failed to load this flock's firmware catalog." }
          },
          Some(list) if list.is_empty() => rsx! {
            p { class: "text-base-content/50 italic text-sm", "No firmware uploaded for this flock yet." }
          },
          Some(list) => rsx! {
            div { class: "overflow-x-auto",
              table { class: "table table-sm",
                thead {
                  tr {
                    th { "Version" }
                    th { "Board" }
                    th { "Size" }
                    th { "sha256" }
                    th {}
                  }
                }
                tbody {
                  for image in list {
                    {
                        let image_id = image.id;
                        let is_current_target = target_target
                            .as_ref()
                            .map(|t| t.sha256 == image.sha256)
                            .unwrap_or(false);
                        let boards_match = boards_compatible(
                            pigeon_board.as_deref(),
                            image.board.as_deref(),
                        );
                        let pigeon_id = pigeon_id.clone();
                        let base_target_config = base_target_config.clone();
                        let target = FirmwareTarget {
                            version: image.version.clone(),
                            size: image.size,
                            sha256: image.sha256.clone(),
                        };
                        let short_sha = image.sha256.chars().take(12).collect::<String>();
                        rsx! {
                          tr {
                            td { class: "font-mono text-xs", "{image.version}" }
                            td { class: "font-mono text-xs",
                              match image.board.as_deref() {
                                Some(board) => rsx! {
                                  "{board}"
                                },
                                None => rsx! {
                                  span { class: "text-base-content/50 italic", "untagged" }
                                },
                              }
                            }
                            td { class: "font-mono text-xs", "{format_bytes(image.size)}" }
                            td { class: "font-mono text-xs", "{short_sha}…" }
                            td {
                              div { class: "flex flex-col items-end gap-1",
                                button {
                                  class: "btn btn-outline btn-xs",
                                  disabled: is_current_target || !boards_match
                                      || assigning_id() == Some(image_id),
                                  title: if !boards_match && !is_current_target {
                                      "This pigeon's board and this image's board must both be set and match exactly before it can be assigned."
                                  },
                                  onclick: move |_| {
                                      let pigeon_id = pigeon_id.clone();
                                      let target = target.clone();
                                      let base_target_config = base_target_config.clone();
                                      async move {
                                          assigning_id.set(Some(image_id));
                                          assign_error.set(None);
                                          match merge_firmware_target(&base_target_config, &target) {
                                              Ok(merged) => {
                                                  let req = PigeonShadowUpdateRequest {
                                                      target_config: merged,
                                                  };
                                                  match api::pigeons::update_shadow(&pigeon_id, &req).await {
                                                      Some(new_shadow) => {
                                                          assigning_id.set(None);
                                                          on_assigned.call(new_shadow);
                                                          on_close.call(());
                                                      }
                                                      None => {
                                                          assigning_id.set(None);
                                                          assign_error
                                                              .set(
                                                                  Some(
                                                                      "Failed to assign firmware -- dovecote rejected it, most likely because this pigeon's board and this image's board aren't both set and matching. Please try again."
                                                                          .to_string(),
                                                                  ),
                                                              );
                                                      }
                                                  }
                                              }
                                              Err(err) => {
                                                  assigning_id.set(None);
                                                  assign_error.set(Some(err));
                                              }
                                          }
                                      }
                                  },
                                  if assigning_id() == Some(image_id) {
                                    span { class: "loading loading-spinner loading-xs" }
                                  } else if is_current_target {
                                    "Assigned"
                                  } else {
                                    "Assign"
                                  }
                                }
                                if !boards_match && !is_current_target {
                                  span { class: "text-warning text-xs", "board mismatch" }
                                }
                              }
                            }
                          }
                        }
                    }
                  }
                }
              }
            }
          },
        }
      }
      BoardDatalist {}
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{boards_compatible, extract_firmware_target, merge_firmware_target};
  use capsules::{FirmwareTarget, JsonString};

  #[test]
  fn boards_match_when_both_set_and_equal() {
    assert!(boards_compatible(
      Some("esp32c6_devkitc/esp32c6/hpcore"),
      Some("esp32c6_devkitc/esp32c6/hpcore")
    ));
  }

  #[test]
  fn boards_mismatch_when_set_but_different() {
    assert!(!boards_compatible(
      Some("esp32c6_devkitc/esp32c6/hpcore"),
      Some("circuitdojo_feather/nrf9160/ns")
    ));
  }

  #[test]
  fn boards_incompatible_when_pigeon_board_unset() {
    assert!(!boards_compatible(
      None,
      Some("esp32c6_devkitc/esp32c6/hpcore")
    ));
  }

  #[test]
  fn boards_incompatible_when_image_board_unset() {
    assert!(!boards_compatible(
      Some("esp32c6_devkitc/esp32c6/hpcore"),
      None
    ));
  }

  #[test]
  fn boards_incompatible_when_both_unset() {
    assert!(!boards_compatible(None, None));
  }

  fn config(raw: &str) -> JsonString {
    JsonString::new(raw.to_string()).expect("test fixture must be valid JSON")
  }

  #[test]
  fn extracts_a_present_firmware_target() {
    let cfg = config(r#"{"firmware":{"version":"0.1.0+0","size":1234,"sha256":"abcd"}}"#);
    let target = extract_firmware_target(&cfg).expect("firmware key is present");
    assert_eq!(target.version, "0.1.0+0");
    assert_eq!(target.size, 1234);
    assert_eq!(target.sha256, "abcd");
  }

  #[test]
  fn returns_none_when_no_firmware_key() {
    let cfg = config(r#"{"telemetry_interval":60}"#);
    assert_eq!(extract_firmware_target(&cfg), None);
  }

  #[test]
  fn merge_preserves_other_keys() {
    let cfg = config(r#"{"telemetry_interval":60}"#);
    let target = FirmwareTarget {
      version: "0.2.0+0".to_string(),
      size: 500,
      sha256: "ffff".to_string(),
    };
    let merged = merge_firmware_target(&cfg, &target).expect("object config must merge");
    assert_eq!(merged["telemetry_interval"], 60);
    assert_eq!(merged["firmware"]["version"], "0.2.0+0");
    assert_eq!(merged["firmware"]["size"], 500);
    assert_eq!(merged["firmware"]["sha256"], "ffff");
  }

  #[test]
  fn merge_overwrites_an_existing_firmware_key() {
    let cfg = config(r#"{"firmware":{"version":"0.1.0+0","size":1,"sha256":"aaaa"}}"#);
    let target = FirmwareTarget {
      version: "0.2.0+0".to_string(),
      size: 2,
      sha256: "bbbb".to_string(),
    };
    let merged = merge_firmware_target(&cfg, &target).expect("object config must merge");
    assert_eq!(merged["firmware"]["version"], "0.2.0+0");
  }

  #[test]
  fn merge_rejects_a_non_object_config() {
    let cfg = config("[1,2,3]");
    let target = FirmwareTarget {
      version: "0.2.0+0".to_string(),
      size: 2,
      sha256: "bbbb".to_string(),
    };
    let result = merge_firmware_target(&cfg, &target);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("isn't a JSON object"));
  }
}
