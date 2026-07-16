use crate::{Route, api};
use capsules::{Flock, FlockCreateRequest};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

#[component]
pub fn Flocks() -> Element {
  rsx! {
    section { id: "flocks",
      div { class: "my-1",
        // Top Navigation / Header
        header { class: "flex flex-col md:flex-row items-center justify-between gap-4 mb-10 grow",
          // Title
          h1 { class: "text-xl font-bold",
            "Flocks ({use_context::<crate::LocalSession>().flocks.read().len()})"
          }

          // Search Bar
          div { class: "grow max-w-2xl mx-auto w-full sm:px-4",
            label { class: "input input-bordered flex items-center gap-2 bg-base-100 w-full",
              input {
                "type": "text",
                class: "grow text-sm",
                placeholder: "Search by flock name",
              }
              Icon {
                width: 16,
                height: 16,
                icon: LdX,
                class: "text-base-content/50 cursor-pointer hover:text-base-content/80",
              }
            }
          }

          // Create Button
          button {
            class: "btn btn-outline btn-primary sm:px-6",
            onclick: move |_| {
                document::eval(r#"document.getElementById("create_flock_modal").showModal();"#);
            },
            "Create Flock"
          }
        }

        // Flocks Grid
        div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6 mb-16",
          for (flock_id , flock) in use_context::<crate::LocalSession>().flocks.read().iter() {
            Link {
              to: Route::Pigeons {
                  flock_id: *flock_id,
              },
              FlockCard { flock: flock.clone() }
            }
          }
        }
      }
      CreateFlockModal {}
    }
  }
}

#[component]
fn FlockCard(flock: Flock) -> Element {
  rsx! {
    div { class: "card bg-base-100 shadow-sm border border-base-200 rounded-md max-w-md card-hover",
      div { class: "card-body",
        // Card Header Row
        div { class: "flex flex-row justify-between",
          h2 { class: "card-title text-secondary font-bold mb-1", "{flock.name}" }
          div { class: "flex items-center gap-2 text-xs text-base-content/60",
            "ID: {flock.id}"
          }
        }

        div { class: "divider my-0" }

        // Card Main Content
        div { class: "flex grow items-center mt-3 gap-8",
          h3 { class: "text-center text-lg font-semibold leading-tight",
            span { class: "m-1 badge badge-lg badge-accent", "{flock.pigeon_ids.len()}" }
            "Pigeons"
          }

          div { class: "flex flex-col space-y-3",
            div { class: "flex flex-col space-y-2 text-sm",
              div {
                span { class: "font-bold", "Plan: " }
                span { class: "text-base-content/70", "{flock.service_plan.to_owned()}" }
              }
              div {
                span { class: "font-bold", "Updated: " }
                span { class: "text-base-content/70",

                  {
                      flock
                          .updated_at
                          .format(
                              time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
                          )
                          .unwrap_or_default()
                  }
                }
              }
              div {
                span { class: "font-bold", "Created: " }
                span { class: "text-base-content/70",
                  {
                      flock
                          .created_at
                          .format(
                              time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
                          )
                          .unwrap_or_default()
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

#[component]
fn CreateFlockModal() -> Element {
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);

  rsx! {
    dialog { class: "modal", id: "create_flock_modal",
      div { class: "modal-box relative max-w-xs md:max-w-sm",
        form { class: "absolute inset-e-2 top-2", method: "dialog",
          button { class: "btn btn-sm btn-circle btn-ghost",
            Icon { icon: LdX, title: "close" }
          }
        }
        div { class: "text-center text-xl font-medium", "Create New Flock" }
        form {
          onsubmit: move |evt: FormEvent| async move {
              evt.prevent_default();
              let mut flock = FlockCreateRequest::default();
              for (key, val) in evt.values() {
                  if let FormValue::Text(val) = val && key == "name" {
                      flock.name = val;
                  }
              }
              is_saving.set(true);
              submit_error.set(None);
              if api::flocks::create(&flock).await.is_some() {
                  is_saving.set(false);
                  document::eval(r#"document.getElementById("create_flock_modal").close();"#);
              } else {
                  is_saving.set(false);
                  submit_error.set(Some("Failed to create flock. Please try again.".to_string()));
              }
          },
          fieldset { class: "fieldset mt-5",
            legend { class: "fieldset-legend", "Name" }
            label { class: "input w-full focus:outline-0",
              input {
                class: "grow focus:outline-0",
                name: "name",
                placeholder: "Name",
                r#type: "text",
                required: true,
              }
            }
          }
          if let Some(err) = submit_error.read().as_ref() {
            p { class: "text-error text-xs mt-2", "⚠️ {err}" }
          }
          div { class: "mt-5 flex items-center justify-end gap-3",
            button {
              class: "btn btn-primary",
              r#type: "submit",
              disabled: is_saving(),
              if is_saving() {
                span { class: "loading loading-spinner loading-sm" }
              } else {
                "Create"
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
