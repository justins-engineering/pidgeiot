use crate::components::{
  ConnectionBadge, ConnectorBadge, FirmwareModal, JsonViewer, LogViewer, PigeonGraphs,
  TelemetryEndpointModal,
};
use crate::helpers::connection_state::{self, ConnectionState};
use crate::{Route, api};
use capsules::{
  CoapConfig, Connector, HttpsConfig, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow,
  PigeonShadowUpdateRequest, PigeonUpdateRequest, TelemetryEndpoint, TelemetryLatest,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowLeft, LdCopy, LdX};
use uuid::Uuid;

#[component]
pub fn PigeonView(flock_id: Uuid, pigeon_id: String) -> Element {
  let id = pigeon_id.clone();
  let mut pigeon_detail: Signal<Option<PigeonDetail>> = use_signal(|| None);
  let mut show_delete_modal = use_signal(|| false);
  let mut show_telemetry_endpoint_modal = use_signal(|| false);
  let mut show_firmware_modal = use_signal(|| false);

  // Connection-state indicator (task #31) -- "last seen" is the newest of
  // three signals this page already fetches (or, for the log chunks,
  // would fetch anyway via LogViewer): the latest telemetry report, the
  // shadow's own updated_at, and the newest received device log chunk.
  // No new backend routes, no extra device traffic.
  let mut telemetry_latest: Signal<Option<Vec<TelemetryLatest>>> = use_signal(|| None);
  let mut latest_log_received: Signal<Option<time::OffsetDateTime>> = use_signal(|| None);

  use_resource(move || {
    let id = id.to_owned();
    async move {
      pigeon_detail.set(api::pigeons::get_detail(&id).await);
    }
  });

  {
    let id = pigeon_id.clone();
    use_resource(move || {
      let id = id.clone();
      async move {
        telemetry_latest.set(api::telemetry::get_latest(&id).await);
      }
    });
  }

  rsx! {
    match pigeon_detail() {
        Some(pd) => {
            let shadow_seen = time::OffsetDateTime::from_unix_timestamp(pd.shadow.updated_at).ok();
            let telemetry_seen = telemetry_latest().and_then(|latest| {
                connection_state::latest_of(latest.iter().map(|t| Some(t.reported_at)))
            });
            let last_seen = connection_state::latest_of([
                shadow_seen,
                telemetry_seen,
                latest_log_received(),
            ]);
            let interval_secs = connection_state::telemetry_interval_secs(
                &pd.shadow.current_config,
            );
            let conn_state: ConnectionState = connection_state::classify(
                last_seen,
                interval_secs,
                time::OffsetDateTime::now_utc(),
            );
            rsx! {
              header { class: "w-full flex flex-row items-center justify-between",
                Link {
                  to: Route::Pigeons { flock_id },
                  class: "btn btn-ghost btn-sm text-base-content/80",
                  Icon {
                    width: 20,
                    height: 20,
                    icon: LdArrowLeft,
                    title: "Pigeons",
                  }
                }
                div { class: "flex flex-row items-center gap-4",
                  h1 { class: "text-4xl font-bold ", "{pd.pigeon.name.as_deref().unwrap_or(\"--\")}" }
                  ConnectionBadge { state: conn_state, last_seen }
                }
                div { class: "flex flex-row items-center gap-2",
                  button {
                    class: "btn btn-outline btn-primary sm:px-6",
                    onclick: move |_| show_firmware_modal.set(true),
                    "Upload Firmware"
                  }
                  button {
                    class: "btn btn-outline btn-error sm:px-6",
                    onclick: move |_| show_delete_modal.set(true),
                    "Delete"
                  }
                }
              }
              div { class: "w-full flex flex-col items-center justify-between gap-4 my-2 md:my-4",
                section { id: "pigeonInfo",
                  PigeonInfo { pigeon: pd.pigeon.clone() }
                }
                section { id: "connectorInfo",
                  ConnectorInfo {
                    pigeon_id: pigeon_id.clone(),
                    connector: pd.pigeon.connector.clone(),
                    token_expires_at: pd.pigeon.token_expires_at,
                  }
                }
                section { id: "telemetryEndpointInfo",
                  TelemetryEndpointInfo {
                    telemetry_endpoint: pd.pigeon.telemetry_endpoint.clone(),
                    show_modal: show_telemetry_endpoint_modal,
                  }
                }
                section { id: "telemetryGraphs",
                  PigeonGraphs { pigeon_id: pigeon_id.clone() }
                }
                section { id: "shadowInfo",
                  ShadowInfo { shadow: pd.shadow.clone() }
                }
                section { id: "logViewer",
                  LogViewer {
                    pigeon_id: pigeon_id.clone(),
                    on_latest_received: move |t| latest_log_received.set(t),
                  }
                }
                section { id: "aclInfo",
                  AclInfo { acl: pd.acl }
                }
                UpdatePigeonModal { flock_id, pigeon: pd.pigeon.clone() }
                EditShadowModal { pigeon_id: pigeon_id.clone(), pigeon_detail }
                if show_firmware_modal() {
                  FirmwareModal {
                    flock_id,
                    pigeon_id: pigeon_id.clone(),
                    shadow: pd.shadow.clone(),
                    on_close: move |_| show_firmware_modal.set(false),
                    on_assigned: move |new_shadow: PigeonShadow| {
                        if let Some(detail) = pigeon_detail.write().as_mut() {
                            detail.shadow = new_shadow;
                        }
                    },
                  }
                }
                if show_telemetry_endpoint_modal() {
                  TelemetryEndpointModal {
                    pigeon_id: pigeon_id.clone(),
                    current: pd.pigeon.telemetry_endpoint.clone(),
                    on_close: move |_| show_telemetry_endpoint_modal.set(false),
                    on_saved: move |endpoint: Option<TelemetryEndpoint>| {
                        if let Some(detail) = pigeon_detail.write().as_mut() {
                            detail.pigeon.telemetry_endpoint = endpoint;
                        }
                        show_telemetry_endpoint_modal.set(false);
                    },
                  }
                }
                if show_delete_modal() {
                  DeletePigeonModal {
                    flock_id,
                    pigeon_id: pigeon_id.clone(),
                    confirm_value: pd.pigeon.name.clone().unwrap_or_else(|| pd.pigeon.id.clone()),
                    on_close: move |_| show_delete_modal.set(false),
                  }
                }
              }
            }
        }
        None => rsx! {
          div { class: "loading loading-spinner text-primary m-10",
            span { "Pigeon not found or loading data..." }
          }
        },
    }
  }
}

#[component]
fn PigeonInfo(pigeon: Pigeon) -> Element {
  let time_format = time::macros::format_description!(
    "[month repr:short] [day padding:none], [year] at [hour]:[minute]:[second] UTC"
  );

  let updated_at = pigeon
    .updated_at
    .format(&time_format)
    .unwrap_or_else(|_| "Invalid Format".to_string());

  let created_at = pigeon
    .created_at
    .format(&time_format)
    .unwrap_or_else(|_| "Invalid Format".to_string());

  let mut copied = use_signal(|| false);

  rsx! {
    div { class: "flex flex-col justify-between items-stretch gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold", "Info" }
        button {
          class: "btn btn-secondary",
          onclick: move |_| {
              document::eval(r#"document.getElementById("update_pigeon_modal").showModal();"#);
          },
          "Edit"
        }
      }

      div { class: "overflow-x-auto",
        table { class: "table",
          tbody {
            tr {
              th { "ID" }
              td {
                div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                  "{pigeon.id}"
                }
              }
              td {
                button {
                  class: "btn btn-square btn-ghost btn-sm",
                  onclick: move |_| {
                      #[cfg(feature = "web")]
                      if let Some(window) = web_sys::window() {
                          let _ = window.navigator().clipboard().write_text(&pigeon.id);
                          copied.set(true);
                      }
                  },
                  Icon { icon: LdCopy }
                }
              }
            }
            tr {
              th { "Flock ID" }
              td {
                div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                  "{pigeon.flock_id}"
                }
              }
              td {
                button {
                  class: "btn btn-square btn-ghost btn-sm",
                  onclick: move |_| {
                      #[cfg(feature = "web")]
                      if let Some(window) = web_sys::window() {
                          let _ = window
                              .navigator()
                              .clipboard()
                              .write_text(&pigeon.flock_id.to_string());
                          copied.set(true);
                      }
                  },
                  Icon { icon: LdCopy }
                }
              }
            }
            tr {
              th { "Serial" }
              td {
                div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                  "{pigeon.serial.as_deref().unwrap_or(\"--\")}"
                }
              }
              td {
                button {
                  class: "btn btn-square btn-ghost btn-sm",
                  onclick: move |_| {
                      #[cfg(feature = "web")]
                      if let Some(window) = web_sys::window() {
                          let _ = window
                              .navigator()
                              .clipboard()
                              .write_text(pigeon.serial.as_deref().unwrap_or("--"));
                          copied.set(true);
                      }
                  },
                  Icon { icon: LdCopy }
                }
              }
            }
            tr {
              th { "Name" }
              td {
                div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                  "{pigeon.name.as_deref().unwrap_or(\"--\")}"
                }
              }
              td {
                button {
                  class: "btn btn-square btn-ghost btn-sm",
                  onclick: move |_| {
                      #[cfg(feature = "web")]
                      if let Some(window) = web_sys::window() {
                          let _ = window
                              .navigator()
                              .clipboard()
                              .write_text(pigeon.name.as_deref().unwrap_or("--"));
                          copied.set(true);
                      }
                  },
                  Icon { icon: LdCopy }
                }
              }
            }
            tr {
              th { "Last Updated" }
              td {
                div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                  "{updated_at}"
                }
              }
              td {
                button {
                  class: "btn btn-square btn-ghost btn-sm",
                  onclick: move |_| {
                      #[cfg(feature = "web")]
                      if let Some(window) = web_sys::window() {
                          let _ = window.navigator().clipboard().write_text(&updated_at);
                          copied.set(true);
                      }
                  },
                  Icon { icon: LdCopy }
                }
              }
            }
            tr {
              th { "Created" }
              td {
                div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                  "{created_at}"
                }
              }
              td {
                button {
                  class: "btn btn-square btn-ghost",
                  onclick: move |_| {
                      #[cfg(feature = "web")]
                      if let Some(window) = web_sys::window() {
                          let _ = window.navigator().clipboard().write_text(&created_at);
                          copied.set(true);
                      }
                  },
                  Icon { icon: LdCopy }
                }
              }
            }
          }
        }
      }
    }
  }
}

#[component]
fn ConnectorInfo(
  pigeon_id: String,
  connector: Connector,
  token_expires_at: time::OffsetDateTime,
) -> Element {
  let time_format = time::macros::format_description!(
    "[month repr:short] [day padding:none], [year] at [hour]:[minute]:[second] UTC"
  );
  let now = time::OffsetDateTime::now_utc();

  let expires_at = token_expires_at
    .format(&time_format)
    .unwrap_or_else(|_| "Invalid Format".to_string());

  let mut copied = use_signal(|| false);
  let mut refreshed_token = use_signal(|| None::<String>);
  let mut is_refreshing = use_signal(|| false);
  let mut refresh_error = use_signal(|| Option::<String>::None);

  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold", "Connector" }
        ConnectorBadge { connector: connector.clone() }
      }

      div { class: "overflow-x-auto",
        table { class: "table",
          tbody {
            match connector {
                Connector::Https(config) => {
                    let endpoint = config.endpoint.clone();
                    rsx! {
                      tr {
                        th { "Protocol" }
                        td { "HTTPS" }
                        td {}
                      }
                      tr {
                        th { "Endpoint" }
                        td {
                          div { class: "font-mono bg-base-200 rounded px-2 w-fit", "{endpoint}" }
                        }
                        td {
                          button {
                            class: "btn btn-square btn-ghost btn-sm",
                            onclick: move |_| {
                                #[cfg(feature = "web")]
                                if let Some(window) = web_sys::window() {
                                    let _ = window.navigator().clipboard().write_text(&endpoint);
                                    copied.set(true);
                                }
                            },
                            Icon { icon: LdCopy }
                          }
                        }
                      }
                    }
                }
                Connector::Coap(config) => {
                    let endpoint = config.endpoint.clone();
                    let tls_psk_identity = config.tls_psk_identity.clone();
                    rsx! {
                      tr {
                        th { "Protocol" }
                        td { "CoAP" }
                        td {}
                      }
                      tr {
                        th { "Endpoint" }
                        td {
                          div { class: "font-mono bg-base-200 rounded px-2 w-fit", "{endpoint}" }
                        }
                        td {
                          button {
                            class: "btn btn-square btn-ghost btn-sm",
                            onclick: move |_| {
                                #[cfg(feature = "web")]
                                if let Some(window) = web_sys::window() {
                                    let _ = window.navigator().clipboard().write_text(&endpoint);
                                    copied.set(true);
                                }
                            },
                            Icon { icon: LdCopy }
                          }
                        }
                      }
                      if let Some(identity) = tls_psk_identity {
                        tr {
                          th { "TLS Identity" }
                          td {
                            div { class: "font-mono bg-base-200 rounded px-2 w-fit", "{identity}" }
                          }
                          td {
                            button {
                              class: "btn btn-square btn-ghost btn-sm",
                              onclick: move |_| {
                                  #[cfg(feature = "web")]
                                  if let Some(window) = web_sys::window() {
                                      let _ = window.navigator().clipboard().write_text(&identity);
                                      copied.set(true);
                                  }
                              },
                              Icon { icon: LdCopy }
                            }
                          }
                        }
                      }
                    }
                }
            }
            tr {
              th { "Token" }
              td {
                if let Some(token) = refreshed_token() {
                  div { class: "flex flex-col gap-2",
                    div { class: "font-mono bg-warning/10 text-warning rounded px-2 py-1 w-fit text-xs",
                      "Copy this token now — it will not be shown again"
                    }
                    div { class: "font-mono bg-base-200 rounded px-2 w-fit break-all",
                      "{token}"
                    }
                  }
                } else {
                  span { class: "text-base-content/50 italic text-sm",
                    "Token hidden for security"
                  }
                }
              }
              td {
                if let Some(token) = refreshed_token() {
                  button {
                    class: "btn btn-success btn-sm",
                    onclick: move |_| {
                        #[cfg(feature = "web")]
                        if let Some(window) = web_sys::window() {
                            let _ = window.navigator().clipboard().write_text(&token);
                            copied.set(true);
                        }
                    },
                    "Copy"
                  }
                } else {
                  button {
                    class: "btn btn-warning btn-sm",
                    disabled: is_refreshing(),
                    onclick: move |_| {
                        let id = pigeon_id.clone();
                        async move {
                            is_refreshing.set(true);
                            refresh_error.set(None);
                            match api::pigeons::refresh_token(&id).await {
                                Some(token) => {
                                    is_refreshing.set(false);
                                    refreshed_token.set(Some(token));
                                }
                                None => {
                                    is_refreshing.set(false);
                                    refresh_error
                                        .set(
                                            Some(
                                                "Failed to refresh token. Please try again."
                                                    .to_string(),
                                            ),
                                        );
                                }
                            }
                        }
                    },
                    if is_refreshing() {
                      span { class: "loading loading-spinner loading-xs" }
                    } else {
                      "Refresh Token"
                    }
                  }
                }
              }
            }
            tr {
              th { "Token Expiry" }
              td {
                div {
                  class: "font-mono bg-base-200 rounded px-2 w-fit",
                  class: if token_expires_at < now { "bg-error" } else { "bg-base-200" },
                  "{expires_at}"
                }
              }
              td {}
            }
          }
        }
      }

      if let Some(err) = refresh_error.read().as_ref() {
        p { class: "text-error text-xs", "⚠️ {err}" }
      }

      if let Some(_token) = refreshed_token() {
        div { class: "flex justify-end",
          button {
            class: "btn btn-ghost btn-sm text-base-content/60",
            onclick: move |_| refreshed_token.set(None),
            "I've Saved the Token"
          }
        }
      }
    }
  }
}

#[component]
fn TelemetryEndpointInfo(
  telemetry_endpoint: Option<TelemetryEndpoint>,
  mut show_modal: Signal<bool>,
) -> Element {
  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold", "Telemetry Endpoint" }
        button {
          class: "btn btn-secondary",
          onclick: move |_| show_modal.set(true),
          if telemetry_endpoint.is_some() {
            "Edit"
          } else {
            "Configure"
          }
        }
      }

      match &telemetry_endpoint {
        Some(endpoint) => rsx! {
          div { class: "overflow-x-auto",
            table { class: "table",
              tbody {
                tr {
                  th { "URL" }
                  td {
                    div { class: "font-mono bg-base-200 rounded px-2 w-fit break-all",
                      "{endpoint.url}"
                    }
                  }
                }
                tr {
                  th { "Database" }
                  td {
                    div { class: "font-mono bg-base-200 rounded px-2 w-fit",
                      "{endpoint.db.as_deref().unwrap_or(\"--\")}"
                    }
                  }
                }
              }
            }
          }
        },
        None => rsx! {
          p { class: "text-base-content/50 italic text-sm",
            "Not configured — telemetry uses the default history store."
          }
        },
      }
    }
  }
}

#[component]
fn ShadowInfo(shadow: PigeonShadow) -> Element {
  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold ", "Shadow" }
        button {
          class: "btn btn-secondary",
          onclick: move |_| {
              document::eval(r#"document.getElementById('edit_shadow_modal').showModal()"#);
          },
          "Edit"
        }
      }
      div { class: "flex flex-col md:flex-row justify-between items-stretch md:items-center gap-4",
        JsonViewer {
          json: shadow.target_config.clone(),
          title: "Target Config [Version: {shadow.target_version}]",
        }
        JsonViewer {
          json: shadow.current_config.clone(),
          title: "Current Config [Version: {shadow.current_version}]",
        }
      }
    }
  }
}

#[component]
fn AclInfo(acl: PigeonAcl) -> Element {
  rsx! {
    div { class: "flex flex-col justify-between items-stretch gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold ", "Access Control List" }
        button { class: "btn btn-disabled", "Edit" }
      }

      div { class: "overflow-x-auto",
        table { class: "table",
          thead {
            tr {
              th { "Entity ID" }
              th { "Role" }
            }
          }
          tbody {
            tr {
              td {
                span { class: "mr-1 badge badge-outline badge-sm", "You" }
                "{acl.entity_id}"
              }
              td {
                span {
                  class: "badge badge-outline",
                  class: if acl.role == "owner" { "badge-primary" },
                  "{acl.role}"
                }
              }
            }
          }
        }
      }
    }
  }
}

/// Client-side-only sanity cap on an uploaded `target_config` JSON file
/// (task #26) -- dovecote's `PUT /pigeons/:id/shadow` enforces no size limit
/// of its own on `target_config`, so this exists purely to give a friendly
/// error instead of stuffing something absurd into the textarea below.
const MAX_SHADOW_UPLOAD_BYTES: u64 = 64 * 1024;

/// Parses an uploaded `target_config` JSON file's text and, on success,
/// returns it pretty-printed for the editor textarea below to preview (task
/// #26 -- "reuse the existing editor ... so the user can eyeball or tweak
/// what the file contained"). Only a JSON *object* is accepted -- arrays and
/// bare scalars would round-trip through `serde_json::Value` fine, but
/// `PigeonShadowUpdateRequest::target_config` is conceptually a config
/// object (see the live textarea's own default of `"{}"`), and silently
/// accepting e.g. `[1,2,3]` here would just surface as a confusing 400 from
/// dovecote later instead of an immediate, specific error now. Pure and
/// synchronous (no `web_sys`) so it's unit-testable without a wasm target.
fn parse_shadow_upload(text: &str) -> Result<String, String> {
  match serde_json::from_str::<serde_json::Value>(text) {
    Ok(value @ serde_json::Value::Object(_)) => {
      serde_json::to_string_pretty(&value).map_err(|err| format!("Invalid JSON: {err}"))
    }
    Ok(_) => Err(
      "The file must contain a JSON object (e.g. {\"key\": \"value\"}), not an array or a bare value."
        .to_string(),
    ),
    Err(err) => Err(format!("Invalid JSON: {err}")),
  }
}

#[cfg(test)]
mod shadow_upload_tests {
  use super::parse_shadow_upload;

  #[test]
  fn accepts_a_json_object_and_pretty_prints_it() {
    // serde_json's `Value::Object` is a `BTreeMap` in this workspace (no
    // `preserve_order` feature), so output keys are always alphabetized
    // regardless of the source file's key order.
    let result = parse_shadow_upload(r#"{"telemetry_interval":60,"logging":"info"}"#);
    assert_eq!(
      result,
      Ok("{\n  \"logging\": \"info\",\n  \"telemetry_interval\": 60\n}".to_string())
    );
  }

  #[test]
  fn accepts_an_empty_object() {
    assert_eq!(parse_shadow_upload("{}"), Ok("{}".to_string()));
  }

  #[test]
  fn rejects_a_json_array() {
    let result = parse_shadow_upload("[1,2,3]");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must contain a JSON object"));
  }

  #[test]
  fn rejects_a_bare_scalar() {
    let result = parse_shadow_upload("42");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must contain a JSON object"));
  }

  #[test]
  fn rejects_a_bare_string() {
    let result = parse_shadow_upload("\"hello\"");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must contain a JSON object"));
  }

  #[test]
  fn rejects_malformed_json() {
    let result = parse_shadow_upload("{not valid json");
    assert!(result.is_err());
    assert!(result.unwrap_err().starts_with("Invalid JSON:"));
  }
}

#[component]
pub fn EditShadowModal(
  pigeon_id: String,
  mut pigeon_detail: Signal<Option<PigeonDetail>>,
) -> Element {
  let mut json_input = use_signal(|| {
    if let Some(detail) = pigeon_detail.read().as_ref() {
      detail.shadow.target_config.to_pretty()
    } else {
      "{}".to_string()
    }
  });

  let mut error_msg = use_signal(|| Option::<String>::None);
  let mut submit_error = use_signal(|| Option::<String>::None);
  let mut is_saving = use_signal(|| false);
  let mut file_error = use_signal(|| Option::<String>::None);
  let mut loaded_file_name = use_signal(|| Option::<String>::None);

  rsx! {
    dialog { class: "modal", id: "edit_shadow_modal",
      div { class: "modal-box relative max-w-2xl bg-base-100 shadow-xl p-6 border border-base-300 rounded-box",

        form { class: "absolute right-4 top-4", method: "dialog",
          button { class: "btn btn-sm btn-circle btn-ghost", "✕" }
        }

        div { class: "text-center text-xl font-medium mb-4 text-secondary",
          "Configure Pigeon Shadow"
        }

        form {
          onsubmit: move |evt: FormEvent| {
              let pigeon_id = pigeon_id.clone();
              let raw_str = json_input.read().clone();

              async move {
                  evt.prevent_default();

                  match serde_json::from_str::<serde_json::Value>(&raw_str) {
                      Ok(json_value) => {
                          error_msg.set(None);
                          submit_error.set(None);
                          is_saving.set(true);

                          let req = PigeonShadowUpdateRequest {
                              target_config: json_value,
                          };

                          match crate::api::pigeons::update_shadow(&pigeon_id, &req).await {
                              Some(new_shadow) => {
                                  if let Some(detail) = pigeon_detail.write().as_mut() {
                                      detail.shadow = new_shadow;
                                  }
                                  is_saving.set(false);
                                  document::eval(
                                      r#"document.getElementById("edit_shadow_modal").close();"#,
                                  );
                              }
                              None => {
                                  is_saving.set(false);
                                  submit_error.set(
                                      Some("Failed to save shadow. Please try again.".to_string()),
                                  );
                              }
                          }
                      }
                      Err(err) => {
                          error_msg.set(Some(format!("Invalid JSON payload: {}", err)));
                      }
                  }
              }
          },

          fieldset { class: "fieldset flex flex-col gap-4",
            div { class: "w-full",
              label { class: "fieldset-legend text-xs font-semibold mb-1 text-base-content/80",
                "Upload from a JSON file (optional)"
              }
              input {
                r#type: "file",
                accept: "application/json,.json",
                class: "file-input file-input-bordered file-input-sm w-full",
                onchange: move |evt: Event<FormData>| {
                    async move {
                        file_error.set(None);
                        loaded_file_name.set(None);
                        let Some(file) = evt.files().into_iter().next() else {
                            return;
                        };
                        if file.size() > MAX_SHADOW_UPLOAD_BYTES {
                            file_error
                                .set(
                                    Some(
                                        format!(
                                            "File is {} KB, which is over the {} KB limit.",
                                            file.size() / 1024,
                                            MAX_SHADOW_UPLOAD_BYTES / 1024,
                                        ),
                                    ),
                                );
                            return;
                        }
                        let text = match file.read_string().await {
                            Ok(text) => text,
                            Err(err) => {
                                file_error.set(Some(format!("Failed to read file: {err}")));
                                return;
                            }
                        };
                        match parse_shadow_upload(&text) {
                            Ok(pretty) => {
                                json_input.set(pretty);
                                error_msg.set(None);
                                loaded_file_name.set(Some(file.name()));
                            }
                            Err(err) => file_error.set(Some(err)),
                        }
                    }
                },
              }
              if let Some(name) = loaded_file_name.read().as_ref() {
                p { class: "text-xs text-success mt-1",
                  "Loaded \"{name}\" into the editor below — review (or tweak) before saving."
                }
              }
              if let Some(err) = file_error.read().as_ref() {
                p { class: "text-error text-xs mt-1", "⚠️ {err}" }
              }
              p { class: "text-xs text-base-content/50 mt-1",
                "Keys your device firmware doesn't understand are ignored on-device."
              }
            }
            div { class: "w-full",
              label { class: "fieldset-legend text-xs font-semibold mb-1 text-base-content/80",
                "Target Configuration Script"
              }

              textarea {
                class: format!(
                    "textarea textarea-bordered font-mono h-72 w-full text-sm p-4 bg-base-200 focus:outline-none transition-colors {}",
                    if error_msg.read().is_some() {
                        "textarea-error"
                    } else {
                        "focus:border-primary/50"
                    },
                ),
                name: "target_config",
                value: "{json_input}",
                placeholder: "{{\n  \"config\": true\n}}",
                oninput: move |e| {
                    json_input.set(e.value());

                    if serde_json::from_str::<serde_json::Value>(&e.value()).is_ok() {
                        error_msg.set(None);
                    } else if e.value().is_empty() {
                        error_msg.set(Some("Configuration cannot be empty.".to_string()));
                    }
                },
              }

              if let Some(err) = error_msg.read().as_ref() {
                label { class: "label py-1",
                  span { class: "label-text-alt text-error font-medium text-xs",
                    "⚠️ {err}"
                  }
                }
              }
              if let Some(err) = submit_error.read().as_ref() {
                label { class: "label py-1",
                  span { class: "label-text-alt text-error font-medium text-xs",
                    "⚠️ {err}"
                  }
                }
              }
            }
          }

          div { class: "mt-6 flex items-center justify-end gap-3",
            form { method: "dialog",
              button { class: "btn btn-ghost btn-sm sm:btn-md", "Cancel" }
            }
            button {
              class: "btn btn-primary shadow-md min-w-[120px]",
              r#type: "submit",
              disabled: error_msg.read().is_some() || is_saving(),
              if is_saving() {
                span { class: "loading loading-spinner loading-sm" }
              } else {
                "Save Changes"
              }
            }
          }
        }
      }

      form { class: "modal-backdrop", method: "dialog",
        button { "close" }
      }
    }
  }
}

/// GitHub-style destructive confirmation: the delete button stays disabled
/// until the user types the pigeon's own name (or, if it has none, its id)
/// back exactly. Rendered conditionally by the caller rather than toggled
/// via a native `<dialog>`, so each open remounts this component and the
/// typed confirmation always starts blank — a stale, already-matching
/// value can't linger across an open/cancel/reopen cycle.
#[component]
fn DeletePigeonModal(
  flock_id: Uuid,
  pigeon_id: String,
  confirm_value: String,
  on_close: EventHandler<()>,
) -> Element {
  let nav = use_navigator();
  let mut local_session = use_context::<crate::LocalSession>();
  let mut input_value = use_signal(String::new);
  let mut is_deleting = use_signal(|| false);
  let mut error_msg = use_signal(|| Option::<String>::None);
  let is_confirmed = input_value() == confirm_value;

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "delete_pigeon_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape && !is_deleting() {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-sm",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          disabled: is_deleting(),
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 { class: "text-lg font-bold text-error", id: "delete_pigeon_title", "Delete Pigeon" }
        p { class: "py-4 text-sm text-base-content/80",
          "This permanently deletes the pigeon and revokes its device credentials. "
          strong { "This cannot be undone." }
        }
        label { class: "fieldset-legend text-xs font-semibold mb-1 block",
          "Type "
          span { class: "font-mono bg-base-200 rounded px-1", "{confirm_value}" }
          " to confirm"
        }
        input {
          class: "input input-bordered w-full text-sm font-mono",
          r#type: "text",
          autocomplete: "off",
          disabled: is_deleting(),
          value: "{input_value}",
          oninput: move |e| input_value.set(e.value()),
          onmounted: move |e| async move {
              let _ = e.set_focus(true).await;
          },
        }
        if let Some(err) = error_msg.read().as_ref() {
          p { class: "text-error text-xs mt-2", "⚠️ {err}" }
        }
        div { class: "modal-action",
          button {
            class: "btn btn-ghost",
            disabled: is_deleting(),
            onclick: move |_| on_close.call(()),
            "Cancel"
          }
          button {
            class: "btn btn-error",
            disabled: !is_confirmed || is_deleting(),
            onclick: move |_| {
                let pigeon_id = pigeon_id.clone();
                async move {
                    is_deleting.set(true);
                    error_msg.set(None);
                    if api::pigeons::delete(&pigeon_id).await.is_some() {
                        if let Some(flock) = local_session.flocks.write().get_mut(&flock_id) {
                            flock.pigeon_ids.retain(|id| id != &pigeon_id);
                        }
                        nav.replace(Route::Pigeons { flock_id });
                    } else {
                        is_deleting.set(false);
                        error_msg
                            .set(
                                Some(
                                    "Failed to delete pigeon. Please try again.".to_string(),
                                ),
                            );
                    }
                }
            },
            if is_deleting() {
              span { class: "loading loading-spinner loading-sm" }
            } else {
              "Delete Pigeon"
            }
          }
        }
      }
    }
  }
}

#[component]
fn UpdatePigeonModal(flock_id: Uuid, pigeon: Pigeon) -> Element {
  let mut selected_connector = use_signal(|| match pigeon.connector {
    Connector::Coap(_) => "Coap".to_string(),
    Connector::Https(_) => "Https".to_string(),
  });
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);

  rsx! {
    dialog { class: "modal", id: "update_pigeon_modal",
      div { class: "modal-box relative max-w-xs md:max-w-sm",
        form { class: "absolute inset-e-2 top-2", method: "dialog",
          button { class: "btn btn-sm btn-circle btn-ghost",
            Icon { icon: LdX, title: "close" }
          }
        }
        div { class: "text-center text-xl font-medium mb-4", "Update Pigeon" }

        form {
          onsubmit: move |evt: FormEvent| {
              let pigeon_id = pigeon.id.to_owned();
              async move {
                  evt.prevent_default();
                  let mut pur = PigeonUpdateRequest {
                      flock_id: Some(flock_id),
                      ..Default::default()
                  };

                  for (key, val) in evt.values() {
                      if let FormValue::Text(val) = val {
                          match key.as_str() {
                              "name" => {
                                  pur.name = if !val.is_empty() { Some(val) } else { None };
                              }
                              "serial" => {
                                  pur.serial = if !val.is_empty() { Some(val) } else { None };
                              }
                              "tags" => {
                                  pur.tags = if !val.is_empty() { Some(val) } else { None };
                              }
                              _ => {}
                          }
                      }
                  }

                  // Build Connector enum from select value
                  pur.connector = match selected_connector.read().as_str() {
                      "Coap" => Some(Connector::Coap(CoapConfig::default())),
                      _ => Some(Connector::Https(HttpsConfig::default())),
                  };

                  is_saving.set(true);
                  submit_error.set(None);
                  if api::pigeons::update(&pigeon_id, &pur).await.is_some() {
                      is_saving.set(false);
                      document::eval(
                          r#"document.getElementById("update_pigeon_modal").close();"#,
                      );
                  } else {
                      is_saving.set(false);
                      submit_error.set(
                          Some("Failed to update pigeon. Please try again.".to_string()),
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
                value: pigeon.name.as_deref().unwrap_or(""),
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
                value: pigeon.serial.as_deref().unwrap_or(""),
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
                option {
                  value: "Https",
                  selected: selected_connector() == "Https",
                  "HTTPS (REST API)"
                }
                option {
                  value: "Coap",
                  selected: selected_connector() == "Coap",
                  "CoAP (TCP)"
                }
              }
            }
            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1",
                "Tags"
              }
              input {
                class: "input input-bordered w-full text-sm",
                name: "tags",
                placeholder: "e.g., Sensor",
                r#type: "text",
                value: pigeon.tags.as_deref().unwrap_or(""),
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
                "Update Pigeon"
              }
            }
          }
        }
      }
      form { class: "modal-backdrop", method: "dialog",
        button { "close" }
      }
    }
  }
}
