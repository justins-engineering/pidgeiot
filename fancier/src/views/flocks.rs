use crate::{Route, api};
use capsules::{CreateFlockPayload, Flock};
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
            "Flocks ({use_context::<crate::LocalSession>().flocks.read().iter().count()})"
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
            if !flock_id.is_empty() {
              Link {
                to: Route::Pigeons {
                    flock_id: flock_id.clone(),
                },
                FlockCard { flock: flock.clone() }
              }
            }
          }
        }
            // Bottom Info Section
      // div { class: "grid grid-cols-1 md:grid-cols-2 gap-8 w-full max-w-4xl mx-auto mt-12",
      //   div { class: "space-y-3",
      //     h3 { class: "text-lg font-bold", "" }
      //     p { class: "text-base-content/70 text-sm leading-relaxed",
      //       ""
      //     }
      //     a {
      //       href: "#",
      //       class: "text-primary text-sm font-medium hover:underline",
      //       ""
      //     }
      //   }
      //   div { class: "space-y-3",
      //     h3 { class: "text-lg font-bold", "" }
      //     p { class: "text-base-content/70 text-sm leading-relaxed",
      //       ""
      //     }
      //     a {
      //       href: "#",
      //       class: "text-primary text-sm font-medium hover:underline",
      //       ""
      //     }
      //   }
      // }
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
            span { class: "m-1 badge badge-lg badge-accent", "{flock.pigeon_count}" }
            "Pigeons"
          }

          div { class: "flex flex-col space-y-3",
            div { class: "flex flex-col space-y-2 text-sm",
              div {
                span { class: "font-bold", "Plan: " }
                span { class: "text-base-content/70",
                  "{flock.service_plan.to_owned().unwrap_or_default()}"
                }
              }
              div {
                span { class: "font-bold", "Updated: " }
                span { class: "text-base-content/70",

                  if let Some(date) = flock.updated_at {
                    {
                        date.format(
                                time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
                            )
                            .unwrap_or_default()
                    }
                  } else {
                    "Unknown Date"
                  }
                
                }
              }
              div {
                span { class: "font-bold", "Created: " }
                span { class: "text-base-content/70",
                  if let Some(date) = flock.created_at {
                    {
                        date.format(
                                time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
                            )
                            .unwrap_or_default()
                    }
                  } else {
                    "Unknown Date"
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
              let mut flock = CreateFlockPayload::default();
              for (key, val) in evt.values() {
                  if let FormValue::Text(val) = val && key == "name" {
                      flock.name = val;
                  }
              }
              api::flocks::create(&flock).await;
              document::eval(r#"document.getElementById("create_flock_modal").close();"#);
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
          div { class: "mt-5 flex items-center justify-end gap-3",
            button { class: "btn btn-primary", r#type: "submit", "Create" }
          }
        }
      }
      form { class: "modal-backdrop", method: "dialog",
        button { "close" }
      }
    }
  }
}
