//! The flock page's config card: the layer every member inherits, how many members took its
//! newest version, and the editor that replaces it. State is built from each write's answer,
//! never from a re-read, which Hyperdrive's cache could serve stale.

use crate::api;
use crate::components::JsonViewer;
use crate::helpers::config_layers::{
  self, MAX_CONFIG_UPLOAD_BYTES, compact_config_bytes, parse_config_upload,
};
use capsules::{FlockConfig, FlockConfigUpdateRequest, MAX_FLOCK_CONFIG_BYTES};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;
use uuid::Uuid;

#[component]
pub fn FlockConfigCard(flock_id: Uuid, can_manage: bool) -> Element {
  let mut config: Signal<Option<Result<FlockConfig, String>>> = use_signal(|| None);
  let mut show_editor = use_signal(|| false);
  let mut busy = use_signal(|| false);
  let mut action_error: Signal<Option<String>> = use_signal(|| None);

  use_resource(move || async move {
    config.set(Some(api::flocks::get_config(flock_id).await));
  });

  let current = config().and_then(Result::ok);
  let has_firmware = current
    .as_ref()
    .is_some_and(|c| config_layers::flock_has_firmware(&c.target_config));

  // Resync and re-push both re-send the layer; only what is sent differs.
  let send = move |target_config: serde_json::Value| async move {
    busy.set(true);
    action_error.set(None);
    let body = FlockConfigUpdateRequest {
      target_config,
      adopt: Vec::new(),
    };
    match api::flocks::put_config(flock_id, &body).await {
      Ok(written) => config.set(Some(Ok(written))),
      Err(message) => action_error.set(Some(message)),
    }
    busy.set(false);
  };

  rsx! {
    div { class: "w-full flex flex-col gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-col items-start gap-4 sm:flex-row sm:items-center sm:justify-between md:px-4",
        div {
          h2 { class: "text-3xl font-bold", "Config" }
          p { class: "text-sm text-base-content/70 max-w-2xl",
            "One config every pigeon in this flock inherits, key by key. A pigeon's own keys "
            "override it, and a pigeon that predates it keeps every key it already held."
          }
        }
        if can_manage {
          if let Some(current) = current.clone() {
            div { class: "flex flex-row flex-wrap gap-2",
              if has_firmware {
                button {
                  class: "btn btn-outline btn-secondary btn-sm",
                  disabled: busy(),
                  title: "Re-sends the flock's firmware under a new target version on every pigeon that takes it",
                  onclick: {
                      let layer = current.target_config.clone();
                      move |_| {
                          let layer = layer.clone();
                          async move {
                              match config_layers::repush_flock_layer(&layer) {
                                  Ok(value) => send(value).await,
                                  Err(err) => action_error.set(Some(err)),
                              }
                          }
                      }
                  },
                  "Re-push firmware"
                }
              }
              button {
                class: "btn btn-outline btn-sm",
                disabled: busy(),
                title: "Sends this same config to every pigeon again, for one that missed it",
                onclick: {
                    let layer = current.target_config.clone();
                    move |_| {
                        let layer = layer.clone();
                        async move {
                            match serde_json::from_str(&layer.to_string()) {
                                Ok(value) => send(value).await,
                                Err(err) => action_error.set(Some(format!("Invalid JSON: {err}"))),
                            }
                        }
                    }
                },
                if busy() {
                  span { class: "loading loading-spinner loading-xs" }
                } else {
                  "Resync"
                }
              }
              button {
                class: "btn btn-secondary btn-sm",
                onclick: move |_| show_editor.set(true),
                "Edit"
              }
            }
          }
        }
      }
      match config() {
          None => rsx! {
            span { class: "loading loading-spinner loading-sm" }
          },
          Some(Err(message)) => rsx! {
            p { class: "text-error text-sm", "Couldn't load this flock's config: {message}" }
          },
          Some(Ok(current)) => rsx! {
            JsonViewer {
              json: current.target_config.clone(),
              title: "Flock Config [Version: {current.config_version}]",
            }
            p { class: "text-sm text-base-content/70 md:px-4",
              if let Some(queued) = current.queued {
                "Sent to {queued} pigeon(s). Each takes it within seconds; reload the page to see the count."
              } else {
                "{current.members_on_version} of {current.members} pigeon(s) on version {current.config_version}."
              }
            }
          },
      }
      if let Some(error) = action_error() {
        p { class: "text-error text-sm md:px-4", "{error}" }
      }
      if show_editor() {
        if let Some(current) = current {
          EditFlockConfigModal {
            flock_id,
            current,
            on_close: move |_| show_editor.set(false),
            on_saved: move |written: FlockConfig| {
                config.set(Some(Ok(written)));
                show_editor.set(false);
            },
          }
        }
      }
    }
  }
}

/// Replaces the flock's layer, typed or loaded from a file, and optionally hands chosen keys back
/// to every member. Rendered from a signal so each open starts from the stored layer.
#[component]
fn EditFlockConfigModal(
  flock_id: Uuid,
  current: FlockConfig,
  on_close: EventHandler<()>,
  on_saved: EventHandler<FlockConfig>,
) -> Element {
  let mut text = use_signal(|| current.target_config.to_pretty());
  let mut adopt: Signal<Vec<String>> = use_signal(Vec::new);
  let mut file_error = use_signal(|| Option::<String>::None);
  let mut loaded_name = use_signal(|| Option::<String>::None);
  let mut submit_error = use_signal(|| Option::<String>::None);
  let mut saving = use_signal(|| false);

  let keys = config_layers::layer_keys(&text.read());
  let selected: Vec<String> = adopt
    .read()
    .iter()
    .filter(|key| keys.contains(key))
    .cloned()
    .collect();
  let checked = config_layers::check_flock_text(&text.read(), &selected);
  let bytes = compact_config_bytes(&text.read());

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "flock_config_modal_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-2xl bg-base-100 shadow-xl p-6 border border-base-300 rounded-box",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute right-4 top-4",
          "aria-label": "Close",
          onclick: move |_| on_close.call(()),
          Icon { width: 16, height: 16, icon: LdX }
        }
        h3 {
          id: "flock_config_modal_title",
          class: "text-center text-xl font-medium mb-4 text-secondary",
          "Flock config"
        }
        fieldset { class: "fieldset flex flex-col gap-4",
          div { class: "w-full",
            label { class: "fieldset-legend text-xs font-semibold mb-1 text-base-content/80",
              "Upload from a JSON file (optional)"
            }
            input {
              r#type: "file",
              accept: "application/json,.json",
              class: "file-input file-input-bordered file-input-sm w-full",
              onchange: move |evt: Event<FormData>| async move {
                  file_error.set(None);
                  loaded_name.set(None);
                  let Some(file) = evt.files().into_iter().next() else {
                      return;
                  };
                  if file.size() > MAX_CONFIG_UPLOAD_BYTES {
                      file_error.set(Some("That file is too large for a config.".to_string()));
                      return;
                  }
                  match file.read_string().await {
                      Ok(contents) => match parse_config_upload(&contents) {
                          Ok(pretty) => {
                              text.set(pretty);
                              loaded_name.set(Some(file.name()));
                          }
                          Err(err) => file_error.set(Some(err)),
                      },
                      Err(err) => file_error.set(Some(format!("Failed to read file: {err}"))),
                  }
              },
            }
            if let Some(name) = loaded_name() {
              p { class: "text-xs text-success mt-1",
                "Loaded \"{name}\" into the editor below. Review it before saving."
              }
            }
            if let Some(err) = file_error() {
              p { class: "text-error text-xs mt-1", "{err}" }
            }
          }
          div { class: "w-full",
            label { class: "fieldset-legend text-xs font-semibold mb-1 text-base-content/80",
              "Config every pigeon inherits"
            }
            textarea {
              class: "textarea textarea-bordered font-mono h-60 w-full text-sm p-4 bg-base-200",
              value: "{text}",
              oninput: move |e| text.set(e.value()),
            }
            if let Some(bytes) = bytes {
              p { class: "text-xs mt-1 text-base-content/60",
                "{bytes} of {MAX_FLOCK_CONFIG_BYTES} bytes"
              }
            }
            if let Err(message) = &checked {
              p { class: "text-error text-sm mt-1", "{message}" }
            }
          }
          if !keys.is_empty() {
            div { class: "w-full",
              label { class: "fieldset-legend text-xs font-semibold mb-1 text-base-content/80",
                "Hand these keys to every pigeon (optional)"
              }
              p { class: "text-xs text-base-content/60 mb-2",
                "A pigeon keeps every key it already holds. Tick a key to clear it from each "
                "pigeon's own config, so all of them take the value above."
              }
              div { class: "flex flex-row flex-wrap gap-x-4 gap-y-1",
                for key in keys.clone() {
                  label { key: "{key}", class: "label cursor-pointer gap-2",
                    input {
                      r#type: "checkbox",
                      class: "checkbox checkbox-sm",
                      checked: selected.contains(&key),
                      onchange: {
                          let key = key.clone();
                          move |_| {
                              let mut chosen = adopt.write();
                              match chosen.iter().position(|k| *k == key) {
                                  Some(at) => {
                                      chosen.remove(at);
                                  }
                                  None => chosen.push(key.clone()),
                              }
                          }
                      },
                    }
                    span { class: "font-mono text-sm", "{key}" }
                  }
                }
              }
            }
          }
          ul { class: "list-disc pl-5 text-xs text-base-content/70 flex flex-col gap-1",
            li { "Removing a key does not reset devices: each keeps the last value it applied." }
            li { "reboot can't be set for a whole flock; reboot pigeons one at a time." }
            li {
              "Each device decodes these keys with its own types, so a value of the wrong type "
              "breaks the config of every pigeon that takes it."
            }
            li {
              "A device refuses a merged config bigger than its build allows (320 bytes by default)."
            }
          }
          if let Some(err) = submit_error() {
            p { class: "text-error text-sm", "{err}" }
          }
        }
        div { class: "mt-6 flex items-center justify-end gap-3",
          button {
            class: "btn btn-ghost btn-sm sm:btn-md",
            onclick: move |_| on_close.call(()),
            "Cancel"
          }
          button {
            class: "btn btn-primary shadow-md min-w-[120px]",
            disabled: checked.is_err() || saving(),
            onclick: {
                let checked = checked.clone();
                let selected = selected.clone();
                move |_| {
                    let checked = checked.clone();
                    let selected = selected.clone();
                    async move {
                        let Ok(target_config) = checked else {
                            return;
                        };
                        saving.set(true);
                        submit_error.set(None);
                        let body = FlockConfigUpdateRequest {
                            target_config,
                            adopt: selected,
                        };
                        match api::flocks::put_config(flock_id, &body).await {
                            Ok(written) => on_saved.call(written),
                            Err(message) => submit_error.set(Some(message)),
                        }
                        saving.set(false);
                    }
                }
            },
            if saving() {
              span { class: "loading loading-spinner loading-sm" }
            } else {
              "Save and send"
            }
          }
        }
      }
      div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
    }
  }
}
