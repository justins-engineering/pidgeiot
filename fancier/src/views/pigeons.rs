use crate::components::{
  BOARD_DATALIST_ID, BoardDatalist, ConnectionBadge, ConnectorBadge, FlockAlerts, FlockGraphs,
};
use crate::helpers::connection_state;
use crate::{Route, api};
use capsules::{CoapConfig, Connector, HttpsConfig, Pigeon, PigeonCreateRequest};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowLeft, LdCopy, LdX};
use std::collections::HashMap;
use wasm_bindgen_futures::JsFuture;

/// How far back the flock pigeon-list looks for a per-pigeon "last seen"
/// (task #31) -- deliberately short and bounded to keep this the only
/// fetch the list makes for the feature (no per-pigeon fan-out, no new
/// routes: one `GET /flocks/:id/telemetry/history` call covers the whole
/// table). A pigeon that hasn't reported within this window shows as
/// offline/unknown even if it reported further back -- the list only
/// needs to distinguish "seen recently" from "not," not produce an exact
/// last-seen age the way the pigeon detail page does.
const LIST_LOOKBACK_HOURS: i64 = 6;

#[component]
pub fn Pigeons(flock_id: uuid::Uuid) -> Element {
  let binding = use_context::<crate::LocalSession>();
  // (pigeon_id, token) — the id rides alongside the token so dismissing
  // the reveal can navigate to the pigeon it belongs to.
  let mut new_token = use_signal(|| None::<(String, String)>);
  let nav = use_navigator();
  let mut last_seen_by_pigeon: Signal<HashMap<String, time::OffsetDateTime>> =
    use_signal(HashMap::new);

  use_resource(move || {
    let flocks = binding.flocks;
    async move {
      let ids_to_fetch = {
        let guard = flocks.read();
        guard.get(&flock_id).map(|flock| flock.pigeon_ids.clone())
      };
      if let Some(pigeon_ids) = ids_to_fetch {
        api::pigeons::list(&pigeon_ids).await;
      };
    }
  });

  use_resource(move || async move {
    let now = time::OffsetDateTime::now_utc();
    let since = now - time::Duration::hours(LIST_LOOKBACK_HOURS);
    if let Some(points) = api::telemetry::get_flock_history(&flock_id, since, now).await {
      last_seen_by_pigeon.set(connection_state::latest_seen_by_pigeon(&points));
    }
  });

  let mut search = use_signal(String::new);
  // Scoped to this flock's own pigeon_ids, not the full LocalSession.pigeons
  // cache -- that cache is shared and additive (api::pigeons::list extends
  // it, never prunes it), so without this filter a pigeon fetched while
  // viewing a *previous* flock stayed visible here after an in-app
  // navigation between flocks (no full page reload to reset client state).
  // Confirmed live: visiting Flock A's pigeon list then Flock B's showed
  // Flock A's pigeon in Flock B's table, complete with a working link into
  // it. flock_id on Pigeon (capsules) is what actually scopes membership.
  let all_pigeons = use_context::<crate::LocalSession>().pigeons;
  let total = all_pigeons
    .read()
    .values()
    .filter(|p| p.flock_id == flock_id)
    .count();
  let filtered: Vec<(String, Pigeon)> = all_pigeons
    .read()
    .iter()
    .filter(|(_, pigeon)| pigeon.flock_id == flock_id)
    .filter(|(_, pigeon)| {
      let query = search.read().to_lowercase();
      query.is_empty()
        || pigeon
          .name
          .as_deref()
          .unwrap_or("")
          .to_lowercase()
          .contains(&query)
        || pigeon
          .serial
          .as_deref()
          .unwrap_or("")
          .to_lowercase()
          .contains(&query)
    })
    .map(|(id, pigeon)| (id.clone(), pigeon.clone()))
    .collect();

  rsx! {
    section { id: "pigeons",
      div { class: "my-1 max-w-7xl mx-auto w-full",

        // Header
        header { class: "flex flex-col md:flex-row items-center justify-between gap-4 mb-10 grow",
          Link {
            to: Route::Flocks {},
            class: "btn btn-ghost btn-sm text-base-content/80",
            Icon {
              width: 20,
              height: 20,
              icon: LdArrowLeft,
              title: "Flocks",
            }
          }
          h1 { class: "text-xl font-bold", "Pigeons ({filtered.len()})" }
          div { class: "grow max-w-2xl mx-auto w-full sm:px-4",
            label { class: "input input-bordered flex items-center gap-2 bg-base-100 w-full",
              input {
                "type": "text",
                class: "grow text-sm",
                placeholder: "Search by serial or name",
                value: "{search}",
                oninput: move |evt| search.set(evt.value()),
              }
              span {
                class: "cursor-pointer",
                onclick: move |_| search.set(String::new()),
                Icon {
                  width: 16,
                  height: 16,
                  icon: LdX,
                  class: "text-base-content/50 hover:text-base-content/80",
                }
              }
            }
          }
          button {
            class: "btn btn-outline btn-primary sm:px-6",
            onclick: move |_| {
                document::eval(r#"document.getElementById("create_pigeon_modal").showModal();"#);
            },
            "Register Pigeon"
          }
        }

        if total == 0 {
          EmptyPigeonsState {}
        } else if filtered.is_empty() {
          div { class: "flex flex-col items-center text-center gap-2 bg-base-100 border border-base-content/10 rounded-box p-12 mb-10 max-w-xl mx-auto",
            h2 { class: "text-lg font-semibold", "No pigeons match \"{search}\"" }
            p { class: "text-base-content/60 max-w-sm",
              "Try a different name or serial, or clear the search to see all pigeons in this flock."
            }
          }
        } else {
          // Pigeons Table
          div { class: "overflow-x-auto rounded-box border border-base-content/10 shadow-sm bg-base-100",
            table { class: "table table-zebra w-full",
              thead {
                tr { class: "bg-base-200/50 text-base-content",
                  th { "Name" }
                  th { "Serial" }
                  th { "Connector" }
                  th { "Status" }
                  th { class: "text-right", "Action" }
                }
              }
              tbody {
                for (id , pigeon) in filtered {
                  tr { class: "hover",
                    td { class: "font-semibold text-primary",
                      "{pigeon.name.as_deref().unwrap_or(\"--\")}"
                    }
                    td { class: "font-mono text-sm text-base-content/70",
                      "{pigeon.serial.as_deref().unwrap_or(\"--\")}"
                    }
                    td { class: "text-sm",
                      ConnectorBadge { connector: pigeon.connector.clone() }
                    }
                    td { class: "text-sm",
                      {
                          let last_seen = last_seen_by_pigeon.read().get(&id).copied();
                          let state = connection_state::classify(
                              last_seen,
                              None,
                              time::OffsetDateTime::now_utc(),
                          );
                          rsx! {
                            ConnectionBadge { state, last_seen }
                          }
                      }
                    }
                    td { class: "text-right",
                      Link {
                        to: Route::PigeonView {
                            flock_id,
                            pigeon_id: id.clone(),
                        },
                        class: "btn btn-ghost btn-xs text-base-content/50",
                        "View →"
                      }
                    }
                  }
                }
              }
            }
          }
        }

        section { id: "flockTelemetryGraphs", class: "mt-10",
          FlockGraphs { flock_id }
        }

        section { id: "flockAlerts", class: "mt-10",
          FlockAlerts { flock_id }
        }

        // One-time token reveal modal. Dismissal — not creation success —
        // is the flow-complete moment: navigating away must never preempt
        // or unmount this modal while the token is still showing.
        if let Some((pigeon_id, token)) = new_token() {
          TokenReveal {
            token,
            on_close: move |_| {
              new_token.set(None);
              nav.replace(Route::PigeonView {
                  flock_id,
                  pigeon_id: pigeon_id.clone(),
              });
            },
          }
        }

        CreatePigeonModal {
          flock_id,
          on_created: move |(pigeon_id, token)| new_token.set(Some((pigeon_id, token))),
        }
      }
    }
  }
}

#[component]
fn EmptyPigeonsState() -> Element {
  rsx! {
    div { class: "flex flex-col items-center text-center gap-3 bg-base-100 border border-base-content/10 rounded-box p-12 mb-10 max-w-xl mx-auto",
      Icon {
        width: 40,
        height: 40,
        icon: LdX,
        class: "text-base-content/30 rotate-45",
      }
      h2 { class: "text-lg font-semibold", "No pigeons in this flock yet" }
      p { class: "text-base-content/60 max-w-sm",
        "Register a pigeon to mint its device keypair and get a one-time bearer token you can bake into firmware."
      }
    }
  }
}

#[component]
fn TokenReveal(token: String, on_close: EventHandler<()>) -> Element {
  let mut copied = use_signal(|| false);
  let mut copy_failed = use_signal(|| false);

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "token_reveal_title",
      tabindex: "-1",
      onkeydown: move |e| {
          if e.key() == Key::Escape {
              on_close.call(());
          }
      },
      onmounted: move |e| async move {
          let _ = e.set_focus(true).await;
      },
      div { class: "modal-box relative max-w-2xl",
        h3 {
          class: "text-lg font-bold text-warning flex items-center gap-2",
          id: "token_reveal_title",
          "🔑 Device Token"
        }
        p { class: "py-4 text-sm text-base-content/80",
          "This token is shown "
          strong { "only once" }
          ". Copy it now and store it securely on your device. It cannot be retrieved later."
        }
        div { class: "bg-base-200 p-4 rounded-lg flex items-center gap-3 border border-warning/30",
          code { class: "font-mono text-xs break-all grow select-all", "{token}" }
          button {
            class: "btn btn-square btn-ghost btn-sm shrink-0",
            onclick: move |_| {
                let token = token.clone();
                async move {
                    #[cfg(feature = "web")]
                    if let Some(window) = web_sys::window() {
                        let result = JsFuture::from(window.navigator().clipboard().write_text(&token))
                            .await;
                        copied.set(result.is_ok());
                        copy_failed.set(result.is_err());
                    }
                }
            },
            if copied() {
              span { class: "text-success text-xs", "Copied!" }
            } else if copy_failed() {
              span { class: "text-error text-xs", "Copy failed — select and copy manually" }
            } else {
              Icon { icon: LdCopy }
            }
          }
        }
        div { class: "modal-action",
          button {
            class: "btn btn-primary",
            onclick: move |_| on_close.call(()),
            "I've Saved the Token"
          }
        }
      }
    }
  }
}
#[component]
fn CreatePigeonModal(flock_id: uuid::Uuid, on_created: EventHandler<(String, String)>) -> Element {
  let mut selected_connector = use_signal(|| "Https".to_string());
  let mut local_session = use_context::<crate::LocalSession>();
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);

  rsx! {
    dialog { class: "modal", id: "create_pigeon_modal",
      div { class: "modal-box relative max-w-xs md:max-w-sm",
        form { class: "absolute inset-e-2 top-2", method: "dialog",
          button { class: "btn btn-sm btn-circle btn-ghost",
            Icon { icon: LdX, title: "close" }
          }
        }
        div { class: "text-center text-xl font-medium mb-4", "Register New Pigeon" }

        form {
          onsubmit: move |evt: FormEvent| {
              let id = flock_id.to_owned();
              async move {
                  evt.prevent_default();
                  let mut pcr = PigeonCreateRequest {
                      flock_id: id,
                      ..Default::default()
                  };

                  for (key, val) in evt.values() {
                      if let FormValue::Text(val) = val {
                          match key.as_str() {
                              "name" => pcr.name = Some(val),
                              "serial" => {
                                  pcr.serial = if !val.is_empty() { Some(val) } else { None };
                              }
                              "board" => {
                                  pcr.board = if !val.is_empty() { Some(val) } else { None };
                              }
                              _ => {}
                          }
                      }
                  }

                  pcr.connector = match selected_connector.read().as_str() {
                      "Coap" => Connector::Coap(CoapConfig::default()),
                      _ => Connector::Https(HttpsConfig::default()),
                  };

                  is_saving.set(true);
                  submit_error.set(None);
                  if let Some((pigeon_id, token)) = api::pigeons::create(&pcr).await {
                      is_saving.set(false);
                      if let Some(flock) = local_session.flocks.write().get_mut(&flock_id) {
                          flock.pigeon_ids.push(pigeon_id.clone());
                      }
                      on_created.call((pigeon_id, token));
                      document::eval(
                          r#"document.getElementById("create_pigeon_modal").close();"#,
                      );
                  } else {
                      is_saving.set(false);
                      submit_error.set(
                          Some("Failed to register pigeon. Please try again.".to_string()),
                      );
                  }
              }
          },

          fieldset { class: "fieldset flex flex-col gap-4",
            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1",
                "Name"
              }
              input {
                class: "input input-bordered w-full text-sm",
                name: "name",
                placeholder: "e.g., Sensor Node Alpha",
                r#type: "text",
                required: true,
              }
            }
            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1",
                "Serial Number"
              }
              input {
                class: "input input-bordered w-full text-sm",
                name: "serial",
                placeholder: "e.g., SN-12345",
                r#type: "text",
              }
            }
            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1",
                "Protocol"
              }
              select {
                class: "select select-bordered w-full text-sm",
                name: "connector",
                onchange: move |evt: Event<FormData>| {
                    for (key, val) in evt.data().values() {
                        if key == "connector" && let FormValue::Text(val) = val {
                            selected_connector.set(val.clone());
                        }
                    }
                },
                option { value: "Https", selected: true, "HTTPS (REST API)" }
                option { value: "Coap", "CoAP (IoT/MQTT)" }
              }
            }
            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1",
                "Board"
              }
              input {
                class: "input input-bordered w-full text-sm font-mono",
                name: "board",
                list: BOARD_DATALIST_ID,
                placeholder: "e.g., circuitdojo_feather/nrf9160/ns",
                r#type: "text",
                autocomplete: "off",
              }
              p { class: "text-xs text-base-content/60 mt-1",
                "Optional — the pigeon's Zephyr board target. Required later to assign firmware (task #20)."
              }
            }
          }
          if let Some(err) = submit_error.read().as_ref() {
            p { class: "text-error text-xs mt-2", "⚠️ {err}" }
          }
          div { class: "mt-6 flex items-center justify-end",
            button {
              class: "btn btn-primary w-full",
              r#type: "submit",
              disabled: is_saving(),
              if is_saving() {
                span { class: "loading loading-spinner loading-sm" }
              } else {
                "Register Device"
              }
            }
          }
        }
      }
      form { class: "modal-backdrop", method: "dialog",
        button { "close" }
      }
      BoardDatalist {}
    }
  }
}
