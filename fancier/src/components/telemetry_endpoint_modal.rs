use crate::api;
use capsules::{PigeonTelemetryEndpointUpdateRequest, TelemetryEndpoint};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

/// Set/clear a pigeon's telemetry forwarding target (task #18's
/// `capsules::TelemetryEndpoint` — a GreptimeDB/InfluxDB-line-protocol HTTP
/// write target the queue consumer forwards to instead of our own history
/// mirror). Rendered conditionally by the caller rather than a native
/// `<dialog>`, per the reset-sensitive modal pattern in CLAUDE.md: it holds
/// an `auth_token` input, and a stale typed token must never linger across
/// an open/cancel/reopen cycle the way it could with a persistent `<dialog>`.
///
/// `current` prefills `url`/`db` but never `auth_token` — same
/// write-only-on-set convention as the device bearer token in
/// `ConnectorInfo`/`refresh_token` (views/pigeon.rs): a previously-set
/// secret is never echoed back by the API, so there's nothing to prefill.
/// The update is a full replace (`PigeonTelemetryEndpointUpdateRequest`
/// carries one `Option<TelemetryEndpoint>`, not per-field patches), so
/// saving with the token field blank clears any existing token — the label
/// below says so rather than implying it's preserved.
///
/// `api::pigeons::update_telemetry_endpoint` targets `PUT
/// /pigeons/:id/telemetry-endpoint` (task #18, landed in dovecote
/// bc1373c). Until that build is deployed to an environment this
/// dashboard build talks to, calls still fail soft (`None`) like any other
/// `fetch_json` caller.
#[component]
pub fn TelemetryEndpointModal(
  pigeon_id: String,
  current: Option<TelemetryEndpoint>,
  on_close: EventHandler<()>,
  on_saved: EventHandler<Option<TelemetryEndpoint>>,
) -> Element {
  let mut url = use_signal(|| current.as_ref().map(|e| e.url.clone()).unwrap_or_default());
  let mut db = use_signal(|| {
    current
      .as_ref()
      .and_then(|e| e.db.clone())
      .unwrap_or_default()
  });
  let mut auth_token = use_signal(String::new);
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);
  let has_existing = current.is_some();
  let clear_pigeon_id = pigeon_id.clone();
  let save_pigeon_id = pigeon_id;

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "telemetry_endpoint_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape && !is_saving() {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-sm",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          disabled: is_saving(),
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 { class: "text-lg font-bold", id: "telemetry_endpoint_title", "Telemetry Endpoint" }
        p { class: "py-2 text-sm text-base-content/70",
          "Forward this pigeon's telemetry to your own GreptimeDB/InfluxDB-compatible endpoint instead of the default history store."
        }

        fieldset { class: "fieldset flex flex-col gap-4 mt-2",
          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Endpoint URL" }
            input {
              class: "input input-bordered w-full text-sm font-mono",
              r#type: "url",
              placeholder: "https://greptime.example.com/v1/influxdb/write",
              value: "{url}",
              oninput: move |e| url.set(e.value()),
            }
          }
          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Database" }
            input {
              class: "input input-bordered w-full text-sm font-mono",
              r#type: "text",
              placeholder: "pigeiot",
              value: "{db}",
              oninput: move |e| db.set(e.value()),
            }
          }
          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Auth Token (optional)" }
            input {
              class: "input input-bordered w-full text-sm font-mono",
              r#type: "password",
              autocomplete: "off",
              placeholder: if has_existing { "blank clears the saved token" } else { "optional" },
              value: "{auth_token}",
              oninput: move |e| auth_token.set(e.value()),
            }
          }
        }

        if let Some(err) = submit_error.read().as_ref() {
          p { class: "text-error text-xs mt-2", "⚠️ {err}" }
        }

        div { class: "modal-action justify-between",
          if has_existing {
            button {
              class: "btn btn-outline btn-error btn-sm",
              disabled: is_saving(),
              onclick: move |_| {
                  let pigeon_id = clear_pigeon_id.clone();
                  async move {
                      is_saving.set(true);
                      submit_error.set(None);
                      let req = PigeonTelemetryEndpointUpdateRequest {
                          telemetry_endpoint: None,
                      };
                      match api::pigeons::update_telemetry_endpoint(&pigeon_id, &req).await {
                          Some(endpoint) => {
                              is_saving.set(false);
                              on_saved.call(endpoint);
                          }
                          None => {
                              is_saving.set(false);
                              submit_error
                                  .set(
                                      Some("Failed to clear endpoint. Please try again.".to_string()),
                                  );
                          }
                      }
                  }
              },
              "Clear"
            }
          } else {
            div {}
          }
          div { class: "flex gap-2",
            button {
              class: "btn btn-ghost btn-sm",
              disabled: is_saving(),
              onclick: move |_| on_close.call(()),
              "Cancel"
            }
            button {
              class: "btn btn-primary btn-sm min-w-[80px]",
              disabled: is_saving() || url.read().is_empty(),
              onclick: move |_| {
                  let pigeon_id = save_pigeon_id.clone();
                  async move {
                      is_saving.set(true);
                      submit_error.set(None);
                      let db_val = db.read().clone();
                      let token_val = auth_token.read().clone();
                      let req = PigeonTelemetryEndpointUpdateRequest {
                          telemetry_endpoint: Some(TelemetryEndpoint {
                              url: url.read().clone(),
                              db: if db_val.is_empty() { None } else { Some(db_val) },
                              auth_token: if token_val.is_empty() { None } else { Some(token_val) },
                          }),
                      };
                      match api::pigeons::update_telemetry_endpoint(&pigeon_id, &req).await {
                          Some(endpoint) => {
                              is_saving.set(false);
                              on_saved.call(endpoint);
                          }
                          None => {
                              is_saving.set(false);
                              submit_error
                                  .set(Some("Failed to save endpoint. Please try again.".to_string()));
                          }
                      }
                  }
              },
              if is_saving() {
                span { class: "loading loading-spinner loading-xs" }
              } else {
                "Save"
              }
            }
          }
        }
      }
    }
  }
}
