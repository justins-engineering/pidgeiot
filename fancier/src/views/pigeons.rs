use crate::{Route, api};
use capsules::Pigeon;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

#[component]
pub fn Pigeons(flock_id: String) -> Element {
  // Note: This API call should ideally be updated to fetch pigeons specific to `flock_id`
  let id = flock_id.clone();
  use_resource(move || {
    let id = id.to_owned();
    async move {
      api::pigeons::list(id).await;
    }
  });

  rsx! {
    section { id: "pigeons",
      div { class: "my-1 max-w-7xl mx-auto w-full",

        // Header
        header { class: "flex flex-col md:flex-row items-center justify-between gap-4 mb-10 grow",
          h1 { class: "text-xl font-bold",
            "Pigeons ({use_context::<crate::LocalSession>().pigeons.read().len()})"
          }

          // Search Bar
          div { class: "grow max-w-2xl mx-auto w-full sm:px-4",
            label { class: "input input-bordered flex items-center gap-2 bg-base-100 w-full",
              input {
                "type": "text",
                class: "grow text-sm",
                placeholder: "Search by serial or name",
              }
              Icon {
                width: 16,
                height: 16,
                icon: LdX,
                class: "text-base-content/50 cursor-pointer hover:text-base-content/80",
              }
            }
          }

          // Register Button
          button {
            class: "btn btn-outline btn-primary sm:px-6",
            onclick: move |_| {
                document::eval(r#"document.getElementById("create_pigeon_modal").showModal();"#);
            },
            "Register Pigeon"
          }
        }

        // Pigeons Table
        div { class: "overflow-x-auto rounded-box border border-base-content/10 shadow-sm bg-base-100",
          table { class: "table table-zebra w-full",
            thead {
              tr { class: "bg-base-200/50 text-base-content",
                th { "Name" }
                th { "Serial" }
                th { "Connector" }
                th { "Location" }
                th { "Last Connected" }
                th { class: "text-right", "Action" }
              }
            }
            tbody {
              for (id , pigeon) in use_context::<crate::LocalSession>().pigeons.read().iter() {
                tr { class: "hover",
                  td { class: "font-semibold text-primary", "{pigeon.name}" }
                  // Fixed: Using .as_deref() prevents moving the String out of the Option
                  td { class: "font-mono text-sm text-base-content/70",
                    "{pigeon.serial.as_deref().unwrap_or(\"--\")}"
                  }
                  td { class: "text-sm",
                    "{pigeon.connector.as_deref().unwrap_or(\"--\")}"
                  }
                  td { class: "text-sm text-base-content/70",
                    "{pigeon.location.as_deref().unwrap_or(\"--\")}"
                  }
                  td { class: "text-sm text-base-content/70",
                    {
                        pigeon
                            .last_connected
                            .and_then(|dt| {
                                dt.format(
                                        time::macros::format_description!(
                                            "[month repr:short] [day padding:none], [year]"
                                        ),
                                    )
                                    .ok()
                            })
                            .unwrap_or_else(|| "Never".to_string())
                    }
                  }
                  td { class: "text-right",
                    Link {
                      to: Route::PigeonView {
                          flock_id: flock_id.clone(),
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
      CreatePigeonModal { flock_id: flock_id.clone() }
    }
  }
}

#[component]
fn CreatePigeonModal(flock_id: String) -> Element {
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
                  let mut pigeon = Pigeon::default();

                  for (key, val) in evt.values() {
                      if let FormValue::Text(val) = val {
                          match key.as_str() {
                              "name" => pigeon.name = val,

                              // Note: Ensure your backend attaches the new Pigeon to `flock_id`
                              "serial" => {
                                  pigeon.serial = if !val.is_empty() {
                                      Some(val)
                                  } else {
                                      None
                                  };
                              }
                              "connector" => pigeon.connector = Some(val),
                              _ => {}
                          }
                      }
                  }
                  info!("{pigeon:?}");
                  api::pigeons::create(id, &pigeon).await;
                  let _ = document::eval(
                      r#"document.getElementById("create_pigeon_modal").close();"#,
                  );
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
                "Connector"
              }
              input {
                class: "input input-bordered w-full text-sm",
                name: "connector",
                placeholder: "HTTPS, MQTT, etc.",
                r#type: "text",
                value: "HTTPS",
              }
            }
          }
          div { class: "mt-6 flex items-center justify-end",
            button { class: "btn btn-primary w-full", r#type: "submit", "Register Device" }
          }
        }
      }
      form { class: "modal-backdrop", method: "dialog",
        button { "close" }
      }
    }
  }
}
