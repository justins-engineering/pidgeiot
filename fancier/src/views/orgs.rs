//! Organizations list + create -- structure mirrors `views/flocks.rs`
//! (header + card grid + native-`<dialog>` create modal), but data is
//! view-local (`use_resource`) rather than `LocalSession`-cached; see
//! `api/orgs.rs`'s module comment.

use crate::{Route, api};
use capsules::OrganizationMembership;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

#[component]
pub fn Orgs() -> Element {
  // Bumped to refetch after a create -- use_resource reruns when a signal
  // it reads changes.
  let refresh = use_signal(|| 0u32);

  let orgs_resource = use_resource(move || async move {
    let _ = refresh();
    api::orgs::list().await
  });

  let orgs: Vec<OrganizationMembership> =
    orgs_resource.read().clone().flatten().unwrap_or_default();

  rsx! {
    section { id: "orgs",
      div { class: "my-1",
        header { class: "flex flex-col md:flex-row items-center justify-between gap-4 mb-10 grow",
          h1 { class: "text-xl font-bold", "Organizations ({orgs.len()})" }
          button {
            class: "btn btn-outline btn-primary sm:px-6",
            onclick: move |_| {
                document::eval(r#"document.getElementById("create_org_modal").showModal();"#);
            },
            "Create Organization"
          }
        }

        if orgs.is_empty() {
          div { class: "flex flex-col items-center text-center gap-3 bg-base-100 border border-base-200 rounded-box p-12 mb-16 max-w-xl mx-auto",
            h2 { class: "text-lg font-semibold", "No organizations yet" }
            p { class: "text-base-content/60 max-w-sm",
              "An organization lets a team share flocks and devices under individual accounts with per-person roles. Create one, then invite your teammates by email."
            }
          }
        } else {
          div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6 mb-16",
            for membership in orgs {
              Link {
                to: Route::OrgView {
                    org_id: membership.organization.id,
                },
                OrgCard { membership: membership.clone() }
              }
            }
          }
        }
      }
      CreateOrgModal { refresh }
    }
  }
}

#[component]
fn OrgCard(membership: OrganizationMembership) -> Element {
  let org = &membership.organization;
  let short_id: String = org.id.to_string().chars().take(8).collect();

  rsx! {
    div { class: "card bg-base-100 shadow-sm border border-base-200 rounded-md max-w-md card-hover",
      div { class: "card-body",
        div { class: "flex flex-row justify-between items-center",
          h2 { class: "card-title text-secondary font-bold mb-1", "{org.name}" }
          span { class: "font-mono bg-base-200 rounded px-2 py-1 text-xs text-base-content/60",
            "{short_id}…"
          }
        }
        div { class: "divider my-0" }
        div { class: "flex items-center justify-between mt-3 text-sm",
          div {
            span { class: "font-bold", "Your role: " }
            span { class: "badge badge-outline badge-accent", "{membership.role}" }
          }
          span { class: "text-base-content/70",
            {
                org
                    .created_at
                    .format(time::macros::format_description!("[year]-[month]-[day]"))
                    .unwrap_or_default()
            }
          }
        }
      }
    }
  }
}

#[component]
fn CreateOrgModal(refresh: Signal<u32>) -> Element {
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);

  rsx! {
    dialog { class: "modal", id: "create_org_modal",
      div { class: "modal-box relative max-w-xs md:max-w-sm",
        form { class: "absolute inset-e-2 top-2", method: "dialog",
          button { class: "btn btn-sm btn-circle btn-ghost",
            Icon { icon: LdX, title: "close" }
          }
        }
        div { class: "text-center text-xl font-medium", "Create Organization" }
        form {
          onsubmit: move |evt: FormEvent| async move {
              evt.prevent_default();
              let mut name = String::new();
              for (key, val) in evt.values() {
                  if let FormValue::Text(val) = val && key == "name" {
                      name = val;
                  }
              }
              is_saving.set(true);
              submit_error.set(None);
              if api::orgs::create(&name).await.is_some() {
                  is_saving.set(false);
                  refresh += 1;
                  document::eval(r#"document.getElementById("create_org_modal").close();"#);
              } else {
                  is_saving.set(false);
                  submit_error
                      .set(Some("Failed to create organization. Please try again.".to_string()));
              }
          },
          fieldset { class: "fieldset mt-5",
            legend { class: "fieldset-legend", "Name" }
            label { class: "input w-full focus:outline-0",
              input {
                class: "grow focus:outline-0",
                name: "name",
                placeholder: "e.g. Pioneer Valley Transit Authority",
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
