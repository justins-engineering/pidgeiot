//! Organization detail: members + roles, pending invites, rename, delete.
//! Follows the codebase's two modal conventions
//! deliberately (see CLAUDE.md): the rename modal is a native `<dialog>`
//! (no reset-sensitive state), while the one-time invite-link reveal is
//! conditional-render (`if let Some(..)`) so it always remounts fresh --
//! it carries a write-once secret, same reasoning as `TokenReveal`.

use crate::{Create, Route, api};
use capsules::{
  BillingPlan, OrgRole, OrganizationBillingOverview, OrganizationDetail, OrganizationInviteCreated,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowLeft, LdCopy, LdX};
use ory_kratos_client_wasm::apis::configuration::Configuration;
use ory_kratos_client_wasm::apis::frontend_api::to_session;
use uuid::Uuid;

#[component]
pub fn OrgView(org_id: Uuid) -> Element {
  let refresh = use_signal(|| 0u32);
  let mut action_error = use_signal(|| Option::<String>::None);
  // One-time invite-link reveal -- conditional-render pattern (see module
  // comment).
  let mut invite_created = use_signal(|| Option::<OrganizationInviteCreated>::None);
  let nav = use_navigator();

  let detail_resource = use_resource(move || async move {
    let _ = refresh();
    api::orgs::detail(org_id).await
  });

  // The caller's own Kratos identity id -- lets the member table mark
  // "(you)" and offer Leave on the caller's own row.
  let me_resource = use_resource(move || async move {
    let config = Configuration::create();
    to_session(&config, None, None, None)
      .await
      .ok()
      .and_then(|s| s.identity.map(|i| i.id))
  });
  let me_id: Option<String> = me_resource.read().clone().flatten();

  let detail = detail_resource.read().clone();

  rsx! {
    section { id: "org", class: "max-w-5xl mx-auto w-full",
      header { class: "flex items-center gap-4 mb-8",
        Link {
          to: Route::Orgs {},
          class: "btn btn-ghost btn-sm text-base-content/80",
          Icon {
            width: 20,
            height: 20,
            icon: LdArrowLeft,
            title: "Organizations",
          }
        }
        match &detail {
          Some(Some(d)) => rsx! {
            h1 { class: "text-xl font-bold grow", "{d.organization.name}" }
            span { class: "badge badge-outline badge-accent", "{d.caller_role}" }
            if d.caller_role.is_manager() {
              button {
                class: "btn btn-outline btn-sm",
                onclick: move |_| {
                    document::eval(r#"document.getElementById("rename_org_modal").showModal();"#);
                },
                "Rename"
              }
            }
            if d.caller_role == OrgRole::Owner {
              button {
                class: "btn btn-outline btn-error btn-sm",
                onclick: move |_| async move {
                    action_error.set(None);
                    match api::orgs::delete(org_id).await {
                        Ok(()) => {
                            nav.replace(Route::Orgs {});
                        }
                        Err(msg) => action_error.set(Some(msg)),
                    }
                },
                "Delete"
              }
            }
          },
          Some(None) => rsx! {
            h1 { class: "text-xl font-bold grow", "Organization" }
          },
          None => rsx! {
            span { class: "loading loading-spinner loading-md" }
          },
        }
      }

      if let Some(err) = action_error.read().as_ref() {
        div { class: "alert alert-error mb-6 text-sm", "⚠️ {err}" }
      }

      match detail {
        Some(Some(d)) => rsx! {
          MembersSection {
            detail: d.clone(),
            me_id,
            refresh,
            action_error,
          }
          if d.caller_role.is_manager() {
            InvitesSection {
              detail: d.clone(),
              refresh,
              action_error,
              invite_created,
            }
          }
          BillingSection {
            org_id,
            caller_role: d.caller_role,
            action_error,
          }
          RenameOrgModal { org_id, refresh }
        },
        Some(None) => rsx! {
          div { class: "flex flex-col items-center text-center gap-2 bg-base-100 border border-base-200 rounded-box p-12 max-w-xl mx-auto",
            h2 { class: "text-lg font-semibold", "Organization unavailable" }
            p { class: "text-base-content/60 max-w-sm",
              "This organization doesn't exist, or you're not a member of it."
            }
          }
        },
        None => rsx! {
          div { class: "flex justify-center p-12",
            span { class: "loading loading-spinner loading-lg" }
          }
        },
      }

      if let Some(created) = invite_created() {
        InviteLinkReveal {
          created,
          on_close: move |_| invite_created.set(None),
        }
      }
    }
  }
}

#[component]
fn MembersSection(
  detail: OrganizationDetail,
  me_id: Option<String>,
  refresh: Signal<u32>,
  action_error: Signal<Option<String>>,
) -> Element {
  let org_id = detail.organization.id;
  let caller_role = detail.caller_role;

  rsx! {
    section { id: "org-members", class: "mb-10",
      h2 { class: "text-lg font-semibold mb-3", "Members ({detail.members.len()})" }
      div { class: "overflow-x-auto rounded-box border border-base-content/10 shadow-sm bg-base-100",
        table { class: "table table-zebra w-full",
          thead {
            tr { class: "bg-base-200/50 text-base-content",
              th { "Member" }
              th { "Role" }
              th { "Joined" }
              th { "Invited by" }
              th { class: "text-right", "Action" }
            }
          }
          tbody {
            for member in detail.members.clone() {
              {
                  let is_me = me_id.as_deref() == Some(member.user_id.to_string().as_str());
                  let member_user_id = member.user_id;
                  let member_role = member.role;
                  rsx! {
                    tr { class: "hover",
                      td {
                        span { class: "font-semibold",
                          "{member.email.as_deref().unwrap_or(\"(no email on record)\")}"
                        }
                        if is_me {
                          span { class: "badge badge-ghost badge-sm ms-2", "you" }
                        }
                        div { class: "font-mono text-xs text-base-content/50", "{member.user_id}" }
                      }
                      td {
                        // Role changes are owner-only (docs/api.md matrix) --
                        // everyone else sees a plain badge.
                        if caller_role == OrgRole::Owner {
                          select {
                            class: "select select-bordered select-sm",
                            onchange: move |evt| async move {
                                let Ok(role) = evt.value().parse::<OrgRole>() else { return };
                                action_error.set(None);
                                match api::orgs::change_role(org_id, member_user_id, role).await {
                                    Ok(_) => refresh += 1,
                                    Err(msg) => {
                                        action_error.set(Some(msg));
                                        refresh += 1;
                                    }
                                }
                            },
                            option { value: "owner", selected: member_role == OrgRole::Owner, "owner" }
                            option { value: "admin", selected: member_role == OrgRole::Admin, "admin" }
                            option {
                              value: "member",
                              selected: member_role == OrgRole::Member,
                              "member"
                            }
                          }
                        } else {
                          span { class: "badge badge-outline", "{member_role}" }
                        }
                      }
                      td { class: "text-sm text-base-content/70",
                        {
                            member
                                .created_at
                                .format(time::macros::format_description!("[year]-[month]-[day]"))
                                .unwrap_or_default()
                        }
                      }
                      td { class: "font-mono text-xs text-base-content/50",
                        {
                            member
                                .invited_by
                                .map(|u| u.to_string().chars().take(8).collect::<String>() + "…")
                                .unwrap_or_else(|| "founder".to_string())
                        }
                      }
                      td { class: "text-right",
                        // Server-enforced rules (docs/api.md): managers may
                        // remove (admins never owners), anyone may leave,
                        // last-owner removal always refused.
                        if is_me || caller_role.is_manager() {
                          button {
                            class: "btn btn-ghost btn-xs text-error",
                            onclick: move |_| async move {
                                action_error.set(None);
                                match api::orgs::remove_member(org_id, member_user_id).await {
                                    Ok(()) => refresh += 1,
                                    Err(msg) => action_error.set(Some(msg)),
                                }
                            },
                            if is_me {
                              "Leave"
                            } else {
                              "Remove"
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
      }
    }
  }
}

#[component]
fn InvitesSection(
  detail: OrganizationDetail,
  refresh: Signal<u32>,
  action_error: Signal<Option<String>>,
  invite_created: Signal<Option<OrganizationInviteCreated>>,
) -> Element {
  let org_id = detail.organization.id;
  let caller_role = detail.caller_role;
  let mut is_sending = use_signal(|| false);

  rsx! {
    section { id: "org-invite", class: "mb-10",
      h2 { class: "text-lg font-semibold mb-3", "Invite a member" }
      form {
        class: "flex flex-col sm:flex-row gap-3 items-stretch sm:items-end bg-base-100 border border-base-content/10 rounded-box p-4 mb-6",
        onsubmit: move |evt: FormEvent| async move {
            evt.prevent_default();
            let mut email = String::new();
            let mut role = OrgRole::Member;
            for (key, val) in evt.values() {
                if let FormValue::Text(val) = val {
                    match key.as_str() {
                        "email" => email = val,
                        "role" => role = val.parse().unwrap_or(OrgRole::Member),
                        _ => {}
                    }
                }
            }
            is_sending.set(true);
            action_error.set(None);
            match api::orgs::create_invite(org_id, &email, role).await {
                Some(created) => {
                    invite_created.set(Some(created));
                    refresh += 1;
                }
                None => {
                    action_error
                        .set(Some("Failed to create invite. Please try again.".to_string()));
                }
            }
            is_sending.set(false);
        },
        fieldset { class: "fieldset grow",
          legend { class: "fieldset-legend", "Email" }
          input {
            class: "input w-full focus:outline-0",
            name: "email",
            r#type: "email",
            placeholder: "teammate@example.com",
            required: true,
          }
        }
        fieldset { class: "fieldset",
          legend { class: "fieldset-legend", "Role" }
          select { class: "select select-bordered", name: "role",
            option { value: "member", selected: true, "member" }
            option { value: "admin", "admin" }
            // Inviting at owner level is itself owner-only (docs/api.md).
            if caller_role == OrgRole::Owner {
              option { value: "owner", "owner" }
            }
          }
        }
        button {
          class: "btn btn-primary",
          r#type: "submit",
          disabled: is_sending(),
          if is_sending() {
            span { class: "loading loading-spinner loading-sm" }
          } else {
            "Send invite"
          }
        }
      }

      h2 { class: "text-lg font-semibold mb-3", "Pending invites ({detail.invites.len()})" }
      if detail.invites.is_empty() {
        p { class: "text-base-content/60 text-sm", "No pending invites." }
      } else {
        div { class: "overflow-x-auto rounded-box border border-base-content/10 shadow-sm bg-base-100",
          table { class: "table table-zebra w-full",
            thead {
              tr { class: "bg-base-200/50 text-base-content",
                th { "Email" }
                th { "Role" }
                th { "Expires" }
                th { class: "text-right", "Action" }
              }
            }
            tbody {
              for invite in detail.invites.clone() {
                {
                    let invite_id = invite.id;
                    rsx! {
                      tr { class: "hover",
                        td { class: "font-semibold", "{invite.email}" }
                        td {
                          span { class: "badge badge-outline", "{invite.role}" }
                        }
                        td { class: "text-sm text-base-content/70",
                          {
                              invite
                                  .expires_at
                                  .format(
                                      time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
                                  )
                                  .unwrap_or_default()
                          }
                        }
                        td { class: "text-right",
                          button {
                            class: "btn btn-ghost btn-xs text-error",
                            onclick: move |_| async move {
                                action_error.set(None);
                                if api::orgs::revoke_invite(org_id, invite_id).await.is_some() {
                                    refresh += 1;
                                } else {
                                    action_error.set(Some("Failed to revoke invite.".to_string()));
                                }
                            },
                            "Revoke"
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
    }
  }
}

/// Navigates the whole tab to a Stripe-hosted page (Checkout or the
/// Billing Portal) -- a full-page handoff on purpose, matching how the
/// Kratos flows leave the SPA: the hosted page 303s back to us when done.
pub(crate) fn redirect_to(url: &str) {
  if let Some(window) = web_sys::window() {
    let _ = window.location().set_href(url);
  }
}

/// Plan, entitlement and usage-vs-allowance for this org, plus the
/// manager-only billing actions. Checkout buttons only render while the
/// org has no live subscription (a second Checkout would create a second
/// subscription alongside the first, not replace it); an entitled org
/// changes tier in place through `PUT /orgs/:id/billing/plan` -- the
/// Stripe portal can't switch a multi-product subscription, so the portal
/// button covers cards, invoices and cancellation only.
#[component]
fn BillingSection(
  org_id: Uuid,
  caller_role: OrgRole,
  action_error: Signal<Option<String>>,
) -> Element {
  let billing_refresh = use_signal(|| 0u32);
  let overview_resource = use_resource(move || async move {
    let _ = billing_refresh();
    api::billing::overview(org_id).await
  });
  let overview = overview_resource.read().clone();

  rsx! {
    section { id: "org-billing", class: "mb-10",
      h2 { class: "text-lg font-semibold mb-3", "Billing" }
      match overview {
        None => rsx! {
          div { class: "flex justify-center p-6",
            span { class: "loading loading-spinner loading-md" }
          }
        },
        Some(None) => rsx! {
          p { class: "text-base-content/60 text-sm",
            "Billing information is unavailable right now."
          }
        },
        Some(Some(o)) => rsx! {
          BillingPanel {
            org_id,
            caller_role,
            action_error,
            billing_refresh,
            overview: o,
          }
        },
      }
    }
  }
}

/// The plain-words confirmation for an in-place tier change, naming the
/// proration direction: an upgrade charges the difference into the next
/// invoice, a downgrade credits it.
fn plan_change_confirm_text(current: BillingPlan, target: BillingPlan) -> String {
  if target > current {
    format!(
      "Switch from {current} to {target} now? The price difference for the rest of this period is charged on your next invoice."
    )
  } else {
    format!(
      "Switch from {current} to {target} now? The unused portion of {current} for the rest of this period is credited on your next invoice."
    )
  }
}

#[component]
fn BillingPanel(
  org_id: Uuid,
  caller_role: OrgRole,
  action_error: Signal<Option<String>>,
  mut billing_refresh: Signal<u32>,
  overview: OrganizationBillingOverview,
) -> Element {
  let mut busy = use_signal(|| false);
  // The refetched overview can lag a plan change by up to a minute
  // (Hyperdrive's read cache), so success gets its own notice instead of
  // relying on the badge flipping immediately.
  let mut change_notice = use_signal(|| Option::<String>::None);
  let o = overview;
  let current_plan = o.plan;
  let usage_pct = if o.included_messages > 0 {
    (o.billable_messages as f64 / o.included_messages as f64 * 100.0).min(100.0)
  } else {
    0.0
  };

  rsx! {
    div { class: "bg-base-100 border border-base-content/10 rounded-box p-4 flex flex-col gap-4",
      div { class: "flex items-center gap-3 flex-wrap",
        span { class: "text-sm text-base-content/60", "Current plan" }
        span { class: "badge badge-primary badge-outline font-mono", "{o.effective_plan}" }
        if o.status != capsules::SubscriptionStatus::None {
          span {
            class: if o.entitled { "badge badge-success badge-sm" } else { "badge badge-ghost badge-sm" },
            "{o.status}"
          }
        }
        if o.plan != o.effective_plan {
          span { class: "text-xs text-base-content/50", "({o.plan} subscription is {o.status})" }
        }
        if o.cancel_at_period_end {
          span { class: "badge badge-warning badge-sm", "cancels at period end" }
        }
      }

      div {
        div { class: "flex justify-between text-sm mb-1",
          span { "Messages this period" }
          span { class: "font-mono", "{o.billable_messages} / {o.included_messages}" }
        }
        progress {
          class: "progress progress-primary w-full",
          value: usage_pct,
          max: 100.0,
        }
      }

      div { class: "text-sm text-base-content/70",
        "Devices: "
        span { class: "font-mono", "{o.device_count}" }
        " of "
        span { class: "font-mono", "{o.included_devices}" }
        " included"
        // Provisioned is the number that hits the cap, connected is the
        // number that gets billed. Showing only the first would leave a
        // customer unable to tell what their invoice will say.
        span { class: "block text-xs text-base-content/50",
          "{o.connected_device_count} connected this period — only these count toward billing"
        }
      }

      if let Some(notice) = change_notice.read().as_ref() {
        div { class: "alert alert-success text-sm", "{notice}" }
      }

      if caller_role.is_manager() {
        div { class: "flex flex-wrap gap-2 items-center",
          if o.entitled {
            for plan in [BillingPlan::Builder, BillingPlan::Growth, BillingPlan::Scale, BillingPlan::Fleet] {
              if plan == current_plan {
                span { class: "badge badge-primary font-mono self-center", "{plan} — current" }
              } else {
                button {
                  class: "btn btn-sm btn-outline",
                  disabled: busy(),
                  onclick: move |_| async move {
                      let confirmed = web_sys::window()
                          .and_then(|w| {
                              w.confirm_with_message(&plan_change_confirm_text(current_plan, plan)).ok()
                          })
                          .unwrap_or(false);
                      if !confirmed {
                          return;
                      }
                      busy.set(true);
                      action_error.set(None);
                      change_notice.set(None);
                      match api::billing::change_plan(org_id, plan).await {
                          Ok(_) => {
                              change_notice
                                  .set(
                                      Some(
                                          format!(
                                              "Plan changed to {plan}. Prorations land on the next invoice; this page can take a minute to catch up.",
                                          ),
                                      ),
                                  );
                              billing_refresh += 1;
                          }
                          Err(msg) => action_error.set(Some(msg)),
                      }
                      busy.set(false);
                  },
                  "Change to {plan}"
                }
              }
            }
            if o.has_billing_account {
              button {
                class: "btn btn-sm btn-primary",
                disabled: busy(),
                onclick: move |_| async move {
                    busy.set(true);
                    action_error.set(None);
                    match api::billing::portal(org_id).await {
                        Ok(url) => redirect_to(&url),
                        Err(msg) => action_error.set(Some(msg)),
                    }
                    busy.set(false);
                },
                "Manage billing"
              }
              span { class: "text-xs text-base-content/50",
                "Card updates, invoices and cancellation happen in the Stripe portal."
              }
            }
          } else {
            for plan in [BillingPlan::Builder, BillingPlan::Growth, BillingPlan::Scale, BillingPlan::Fleet] {
              button {
                class: "btn btn-sm btn-outline",
                disabled: busy(),
                onclick: move |_| async move {
                    busy.set(true);
                    action_error.set(None);
                    match api::billing::checkout(org_id, plan).await {
                        Ok(url) => redirect_to(&url),
                        Err(msg) => action_error.set(Some(msg)),
                    }
                    busy.set(false);
                },
                "Upgrade to {plan}"
              }
            }
            if o.has_billing_account {
              button {
                class: "btn btn-sm btn-ghost",
                disabled: busy(),
                onclick: move |_| async move {
                    busy.set(true);
                    action_error.set(None);
                    match api::billing::portal(org_id).await {
                        Ok(url) => redirect_to(&url),
                        Err(msg) => action_error.set(Some(msg)),
                    }
                    busy.set(false);
                },
                "Billing history"
              }
            }
          }
        }
      }
    }
  }
}

/// One-time reveal of a freshly-minted invite link -- conditional-render
/// (never a persistent `<dialog>`) because the token inside is write-once:
/// it will never be shown again after dismissal, exactly like the device
/// `TokenReveal`.
#[component]
fn InviteLinkReveal(created: OrganizationInviteCreated, on_close: EventHandler<()>) -> Element {
  let invite_url = created.invite_url.clone();

  rsx! {
    div { class: "modal modal-open",
      div { class: "modal-box max-w-lg",
        h3 { class: "text-lg font-bold", "Invite created" }
        p { class: "py-2 text-sm text-base-content/70",
          "An email is on its way to "
          span { class: "font-semibold", "{created.invite.email}" }
          ". You can also share this link directly — it won't be shown again:"
        }
        div { class: "flex items-center gap-2 bg-base-200 rounded-box p-3 font-mono text-xs break-all",
          span { class: "grow", "{invite_url}" }
          button {
            class: "btn btn-square btn-ghost btn-xs",
            title: "Copy invite link",
            onclick: move |_| {
                if let Some(window) = web_sys::window() {
                    let _ = window.navigator().clipboard().write_text(&invite_url);
                }
            },
            Icon { icon: LdCopy, width: 14, height: 14 }
          }
        }
        p { class: "pt-2 text-xs text-base-content/50",
          "Single-use, expires in 7 days. Whoever opens it and signs in joins as {created.invite.role}."
        }
        div { class: "modal-action",
          button { class: "btn btn-primary", onclick: move |_| on_close.call(()), "Done" }
        }
      }
    }
  }
}

#[component]
fn RenameOrgModal(org_id: Uuid, refresh: Signal<u32>) -> Element {
  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);

  rsx! {
    dialog { class: "modal", id: "rename_org_modal",
      div { class: "modal-box relative max-w-xs md:max-w-sm",
        form { class: "absolute inset-e-2 top-2", method: "dialog",
          button { class: "btn btn-sm btn-circle btn-ghost",
            Icon { icon: LdX, title: "close" }
          }
        }
        div { class: "text-center text-xl font-medium", "Rename Organization" }
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
              if api::orgs::rename(org_id, &name).await.is_some() {
                  is_saving.set(false);
                  refresh += 1;
                  document::eval(r#"document.getElementById("rename_org_modal").close();"#);
              } else {
                  is_saving.set(false);
                  submit_error.set(Some("Failed to rename organization.".to_string()));
              }
          },
          fieldset { class: "fieldset mt-5",
            legend { class: "fieldset-legend", "New name" }
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
                "Rename"
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
