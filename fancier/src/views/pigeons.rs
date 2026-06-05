use crate::{Route, api};
use capsules::PigeonCreateRequest;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowLeft, LdX};

#[component]
pub fn Pigeons(flock_id: uuid::Uuid) -> Element {
  let binding = use_context::<crate::LocalSession>();

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
                th { class: "text-right", "Action" }
              }
            }
            tbody {
              for (id , pigeon) in use_context::<crate::LocalSession>().pigeons.read().iter() {
                tr { class: "hover",
                  td { class: "font-semibold text-primary",
                    "{pigeon.name.as_deref().unwrap_or(\"--\")}"
                  }
                  // Fixed: Using .as_deref() prevents moving the String out of the Option
                  td { class: "font-mono text-sm text-base-content/70",
                    "{pigeon.serial.as_deref().unwrap_or(\"--\")}"
                  }
                  td { class: "text-sm", "{pigeon.connector}" }
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
      CreatePigeonModal { flock_id }
    }
  }
}

#[component]
fn CreatePigeonModal(flock_id: uuid::Uuid) -> Element {
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
                  let mut pcr: PigeonCreateRequest = PigeonCreateRequest {
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
                              "connector" => pcr.connector = val,
                              _ => {}
                          }
                      }
                  }
                  if api::pigeons::create(&pcr).await.is_some() {
                      document::eval(
                          r#"document.getElementById("create_pigeon_modal").close();"#,
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
