//! Organizations list + create -- structure mirrors `views/flocks.rs`
//! (header + card grid + native-`<dialog>` create modal), reading the
//! `LocalSession.orgs` cache that `api/orgs.rs` keeps current from each
//! mutation's response.

use crate::{Route, UpgradeIntent, api};
use capsules::{BillingPlan, OrganizationCreateRequest, OrganizationMembership, TaxIdType};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdTriangleAlert, LdX};

/// The cache in the order `GET /orgs` lists it: oldest first, with the id
/// as a tiebreak so two orgs created in the same instant keep a stable
/// order between renders.
fn sorted_memberships(
  orgs: &std::collections::HashMap<uuid::Uuid, OrganizationMembership>,
) -> Vec<OrganizationMembership> {
  let mut memberships: Vec<OrganizationMembership> = orgs.values().cloned().collect();
  memberships.sort_by(|a, b| {
    a.organization
      .created_at
      .cmp(&b.organization.created_at)
      .then_with(|| a.organization.id.cmp(&b.organization.id))
  });
  memberships
}

/// Why this page opened, for a visitor sent here by a pricing-page
/// upgrade. Only someone who manages more than one organization arrives
/// this way -- the plan has to attach to one of them, and the org's own
/// Billing section is where that choice is made.
fn upgrade_intent_message(plan: BillingPlan) -> String {
  const HEAD: &str = "Open the organization you want on the ";
  const TAIL: &str = " plan and use Upgrade in its Billing section.";
  let plan = plan.as_str();
  let mut message = String::with_capacity(HEAD.len() + plan.len() + TAIL.len());
  message.push_str(HEAD);
  message.push_str(plan);
  message.push_str(TAIL);
  message
}

#[component]
pub fn Orgs() -> Element {
  let local_session = use_context::<crate::LocalSession>();
  let mut upgrade_intent = use_context::<UpgradeIntent>().0;
  // The notice explains one arrival, so it must not outlive the visit it
  // was set for: coming back to this page later is not that arrival.
  use_drop(move || upgrade_intent.set(None));
  let load_failed = (local_session.orgs_load_failed)();
  let orgs = sorted_memberships(&local_session.orgs.read());

  rsx! {
    section { id: "orgs",
      div { class: "my-1",
        // In flow rather than the shared `Alert`: that one is sticky, and
        // here it would sit on top of the header it pushes down.
        if let Some(plan) = upgrade_intent() {
          div { role: "alert", class: "alert alert-info alert-soft mb-6",
            "{upgrade_intent_message(plan)}"
          }
        }
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

        if orgs.is_empty() && load_failed {
          OrgsUnavailableState {}
        } else if orgs.is_empty() {
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
      CreateOrgModal {}
    }
  }
}

/// Shown instead of the empty state when the sign-in load of the list
/// failed: an empty map on its own cannot tell "no organizations" from
/// "the request never came back", and telling someone they have none when
/// the API was simply unreachable invites them to create a duplicate.
#[component]
fn OrgsUnavailableState() -> Element {
  rsx! {
    div { class: "flex flex-col items-center text-center gap-3 bg-base-100 border border-base-200 rounded-box p-12 mb-16 max-w-xl mx-auto",
      Icon {
        width: 40,
        height: 40,
        icon: LdTriangleAlert,
        class: "text-warning",
      }
      h2 { class: "text-lg font-semibold", "Couldn't load your organizations" }
      p { class: "text-base-content/60 max-w-sm",
        "This is a problem reaching the API, not an empty account. Reload to try again."
      }
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

/// On success the new org is already in `LocalSession.orgs` (put there by
/// `api::orgs::create` from the response), so closing the dialog is all
/// that is left to do -- the list behind it re-renders from the cache.
#[component]
fn CreateOrgModal() -> Element {
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);
  // Only the select needs a signal -- the text inputs are read from the
  // submitted form, same as `name`.
  let mut tax_id_type = use_signal(|| TaxIdType::EuVat);

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
              let mut business_name = String::new();
              let mut tax_id = String::new();
              for (key, val) in evt.values() {
                  if let FormValue::Text(val) = val {
                      match key.as_str() {
                          "name" => name = val,
                          "business_name" => business_name = val,
                          "tax_id" => tax_id = val,
                          _ => {}
                      }
                  }
              }
              let tax_id = tax_id.trim().to_string();
              let request = OrganizationCreateRequest {
                  name,
                  business_name: Some(business_name),
                  tax_id: if tax_id.is_empty() { None } else { Some(tax_id.clone()) },
                  tax_id_type: if tax_id.is_empty() { TaxIdType::None } else { tax_id_type() },
              };
              is_saving.set(true);
              submit_error.set(None);
              match api::orgs::create(&request).await {
                  Ok(_) => {
                      is_saving.set(false);
                      document::eval(r#"document.getElementById("create_org_modal").close();"#);
                  }
                  Err(msg) => {
                      is_saving.set(false);
                      submit_error.set(Some(msg));
                  }
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
          // Optional, and behind a disclosure, because an org is also how a
          // single person groups their own devices. The fields are here at
          // all because this is the moment the billing entity comes into
          // existence, and a VAT registration entered now is one nobody has
          // to remember to add before the first invoice.
          details { class: "collapse collapse-arrow bg-base-200 mt-4",
            summary { class: "collapse-title text-sm font-medium min-h-0 py-2",
              "Business details (optional)"
            }
            div { class: "collapse-content flex flex-col gap-3",
              label { class: "input w-full focus:outline-0",
                input {
                  class: "grow focus:outline-0",
                  name: "business_name",
                  placeholder: "Registered business name",
                  r#type: "text",
                }
              }
              div { class: "flex gap-2",
                // Selection on the option rather than a `value` on the
                // select, for the reason given on the same control in
                // `views/org.rs`.
                select {
                  class: "select select-bordered select-sm",
                  onchange: move |e| {
                      if let Ok(parsed) = e.value().parse::<TaxIdType>() {
                          tax_id_type.set(parsed);
                      }
                  },
                  for kind in TaxIdType::ALL.iter().filter(|kind| **kind != TaxIdType::None) {
                    option {
                      value: "{kind.as_str()}",
                      selected: *kind == tax_id_type(),
                      "{kind.label()}"
                    }
                  }
                }
                label { class: "input grow focus:outline-0",
                  input {
                    class: "grow focus:outline-0 font-mono",
                    name: "tax_id",
                    placeholder: "Tax ID",
                    r#type: "text",
                  }
                }
              }
              p { class: "text-xs text-base-content/50",
                "EU VAT IDs are checked against VIES. You can add or change these later."
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

#[cfg(test)]
mod upgrade_intent_message_tests {
  use super::upgrade_intent_message;
  use capsules::BillingPlan;

  #[test]
  fn names_the_plan_and_where_to_apply_it() {
    let message = upgrade_intent_message(BillingPlan::Growth);
    assert_eq!(
      message,
      "Open the organization you want on the growth plan and use Upgrade in its Billing section."
    );
  }

  // The capacity is the whole point of building it this way: a resize
  // would mean the parts were mis-counted.
  #[test]
  fn message_is_one_allocation() {
    for plan in [
      BillingPlan::Builder,
      BillingPlan::Growth,
      BillingPlan::Scale,
      BillingPlan::Fleet,
    ] {
      let message = upgrade_intent_message(plan);
      assert_eq!(message.len(), message.capacity(), "{plan}");
    }
  }
}
