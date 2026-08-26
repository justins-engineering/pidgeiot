//! Org invite acceptance -- the landing page for
//! `<ROOT_URL>/invite?token=<token>` links minted by `POST
//! /orgs/:org_id/invites`.
//!
//! Deliberately a PUBLIC route (not under `AuthGuard`): an invitee usually
//! has no account yet, and `AuthGuard` would bounce them to `/unauthorized`
//! and lose the link. Instead this view reads the session state itself --
//! unauthenticated visitors get sign-in/register links and re-open the
//! emailed link once signed in; authenticated visitors get an explicit
//! "Accept invitation" button (button-driven, never auto-consumed on
//! render: the token is single-use, and a stray prefetch/reload must not
//! burn it).
//!
//! The `token` query param is read from the ADDRESS BAR via
//! `url_query_param` first; the route prop is only the SSG-side fallback.
//! Skipping this would hand the prerendered `token: None` to every
//! full-page load of an invite link.

use crate::helpers::url_query_param;
use crate::models::AuthState;
use crate::{Route, Session, api};
use capsules::OrganizationMembership;
use dioxus::prelude::*;

#[component]
pub fn InviteAccept(token: Option<String>) -> Element {
  // Address bar first (SSG hydration drops query-derived props -- see
  // module comment), route prop only as fallback.
  let token = url_query_param("token").or(token);
  let session = use_context::<Session>();

  let mut is_accepting = use_signal(|| false);
  let mut outcome = use_signal(|| Option::<Result<OrganizationMembership, String>>::None);

  rsx! {
    section { id: "invite", class: "max-w-xl mx-auto w-full py-12",
      div { class: "bg-base-100 border border-base-200 rounded-box p-8 flex flex-col gap-4 text-center",
        h1 { class: "text-xl font-bold", "Organization invitation" }

        match (token.clone(), (session.state)()) {
          (None, _) => rsx! {
            p { class: "text-base-content/70",
              "This invite link is missing its token. Ask the person who invited you to send a fresh link."
            }
          },
          (Some(_), AuthState::Pending) => rsx! {
            span { class: "loading loading-spinner loading-md mx-auto" }
            p { class: "text-base-content/60 text-sm", "Checking your session..." }
          },
          (Some(_), AuthState::Unauthenticated) => rsx! {
            p { class: "text-base-content/70",
              "You've been invited to join an organization on PidgeIoT. Sign in (or create an account) first, then open this invite link again from your email."
            }
            div { class: "flex justify-center gap-3",
              Link {
                class: "btn btn-primary",
                to: Route::LoginFlow { flow: None },
                "Sign in"
              }
              Link {
                class: "btn btn-outline",
                to: Route::RegisterFlow { flow: None },
                "Create account"
              }
            }
          },
          (Some(tok), AuthState::Authenticated) => rsx! {
            match outcome() {
              None => rsx! {
                p { class: "text-base-content/70",
                  "You've been invited to join an organization. Accepting adds this account to the organization's member list."
                }
                button {
                  class: "btn btn-primary mx-auto",
                  disabled: is_accepting(),
                  onclick: move |_| {
                      let tok = tok.clone();
                      async move {
                          is_accepting.set(true);
                          let result = api::orgs::accept_invite(&tok).await;
                          outcome.set(Some(result));
                          is_accepting.set(false);
                      }
                  },
                  if is_accepting() {
                    span { class: "loading loading-spinner loading-sm" }
                  } else {
                    "Accept invitation"
                  }
                }
              },
              Some(Ok(membership)) => rsx! {
                p { class: "text-success font-semibold",
                  "You're in! Welcome to {membership.organization.name}."
                }
                Link {
                  class: "btn btn-primary mx-auto",
                  to: Route::OrgView {
                      org_id: membership.organization.id,
                  },
                  "Open the organization"
                }
              },
              Some(Err(msg)) => rsx! {
                p { class: "text-error text-sm", "⚠️ {msg}" }
                Link { class: "btn btn-outline mx-auto", to: Route::Orgs {}, "My organizations" }
              },
            }
          },
        }
      }
    }
  }
}
