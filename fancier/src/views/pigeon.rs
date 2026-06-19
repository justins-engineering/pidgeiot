use crate::components::{ConnectorBadge, JsonViewer};
use crate::{Route, api};
use capsules::{
  CoapConfig, Connector, HttpsConfig, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow,
  PigeonUpdateRequest,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowLeft, LdCopy, LdX};
use uuid::Uuid;

#[component]
pub fn PigeonView(flock_id: Uuid, pigeon_id: String) -> Element {
  let id = pigeon_id.clone();
  let mut pigeon_detail: Signal<Option<PigeonDetail>> = use_signal(|| None);

  use_resource(move || {
    let id = id.to_owned();
    async move {
      pigeon_detail.set(api::pigeons::get_detail(&id).await);
    }
  });

  rsx! {
    match pigeon_detail() {
        Some(pd) => {
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
                h1 { class: "text-4xl font-bold ", "{pd.pigeon.name.as_deref().unwrap_or(\"--\")}" }
                button { class: "btn btn-outline btn-primary sm:px-6", "Upload Firmware" }
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
                section { id: "shadowInfo",
                  ShadowInfo { shadow: pd.shadow }
                }
                section { id: "aclInfo",
                  AclInfo { acl: pd.acl }
                }
                UpdatePigeonModal { flock_id, pigeon: pd.pigeon }
              }
            }
        }
        None => rsx! {
          div { class: "alert alert-warning shadow-lg",
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
                    let dtls_psk_identity = config.dtls_psk_identity.clone();
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
                      if let Some(identity) = dtls_psk_identity {
                        tr {
                          th { "DTLS Identity" }
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
                    onclick: move |_| {
                        let id = pigeon_id.clone();
                        async move {
                            if let Some(token) = api::pigeons::refresh_token(&id).await {
                                refreshed_token.set(Some(token));
                            }
                        }
                    },
                    "Refresh Token"
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
fn ShadowInfo(shadow: PigeonShadow) -> Element {
  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold ", "Shadow" }
        button {
          class: "btn btn-secondary",
          onclick: move |_| {
              document::eval(r#"document.getElementById("update_pigeon_modal").showModal();"#);
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

#[component]
fn UpdatePigeonModal(flock_id: Uuid, pigeon: Pigeon) -> Element {
  let mut selected_connector = use_signal(|| match pigeon.connector {
    Connector::Coap(_) => "Coap".to_string(),
    Connector::Https(_) => "Https".to_string(),
  });

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

                  if api::pigeons::update(&pigeon_id, &pur).await.is_some() {
                      document::eval(
                          r#"document.getElementById("update_pigeon_modal").close();"#,
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
          div { class: "mt-6 flex items-center justify-end",
            button { class: "btn btn-primary w-full", r#type: "submit", "Update Pigeon" }
          }
        }
      }
      form { class: "modal-backdrop", method: "dialog",
        button { "close" }
      }
    }
  }
}
