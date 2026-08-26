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
//! `url_query_param`, and only after mount -- see `InviteToken` for why
//! neither half of that is optional on a prerendered page.

use crate::helpers::url_query_param;
use crate::models::AuthState;
use crate::{Route, Session, api};
use capsules::OrganizationMembership;
use dioxus::prelude::*;

/// What the page knows about the token it was opened with.
///
/// `Unread` is what the SSG prerender bakes into the static page, and it
/// is also what the first client render must produce: a prerendered page
/// hydrates by adopting the markup it was served, so a first render that
/// picks a different arm of the card (because it already read `?token=`
/// off the address bar) leaves that arm's nodes mounted as siblings of
/// the card rather than inside it. The token is therefore read from a
/// `use_future`, which never runs during the synchronous prerender, and
/// the signal leaving `Unread` is a real reactive update that patches the
/// card in place.
#[derive(Clone, Debug, PartialEq)]
enum InviteToken {
  Unread,
  Missing,
  Present(String),
}

/// The address bar wins over the route prop. The prop is whatever the
/// prerender baked (`None`, since the static page has no query string) and
/// a full-page load hydrates that back regardless of the real URL; on
/// client-side navigation the two agree and the prop is redundant. It is
/// still consulted last for the native build, which has no address bar.
fn resolve_invite_token(
  from_address_bar: Option<String>,
  from_route: Option<String>,
) -> InviteToken {
  match from_address_bar.or(from_route) {
    Some(token) if !token.is_empty() => InviteToken::Present(token),
    _ => InviteToken::Missing,
  }
}

#[component]
pub fn InviteAccept(token: Option<String>) -> Element {
  let session = use_context::<Session>();

  let mut invite_token = use_signal(|| InviteToken::Unread);
  use_future(move || {
    let from_route = token.clone();
    async move {
      invite_token.set(resolve_invite_token(url_query_param("token"), from_route));
    }
  });

  let mut is_accepting = use_signal(|| false);
  let mut outcome = use_signal(|| Option::<Result<OrganizationMembership, String>>::None);

  rsx! {
    section { id: "invite", class: "max-w-xl mx-auto w-full py-12",
      div { class: "bg-base-100 border border-base-200 rounded-box p-8 flex flex-col gap-4 text-center",
        h1 { class: "text-xl font-bold", "Organization invitation" }

        match (invite_token(), (session.state)()) {
          (InviteToken::Unread, _) | (InviteToken::Present(_), AuthState::Pending) => rsx! {
            span { class: "loading loading-spinner loading-md mx-auto" }
            p { class: "text-base-content/60 text-sm", "Checking your invitation..." }
          },
          (InviteToken::Missing, _) => rsx! {
            p { class: "text-base-content/70",
              "This invite link is missing its token. Ask the person who invited you to send a fresh link."
            }
          },
          (InviteToken::Present(_), AuthState::Unauthenticated) => rsx! {
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
          (InviteToken::Present(tok), AuthState::Authenticated) => rsx! {
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
                div { class: "flex justify-center gap-3",
                  Link {
                    class: "btn btn-primary",
                    to: Route::OrgView {
                        org_id: membership.organization.id,
                    },
                    "Open the organization"
                  }
                  Link { class: "btn btn-outline", to: Route::Orgs {}, "My organizations" }
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_address_bar_wins_over_the_route_prop() {
    assert_eq!(
      resolve_invite_token(Some("from-bar".into()), Some("from-route".into())),
      InviteToken::Present("from-bar".into())
    );
  }

  #[test]
  fn the_route_prop_is_only_a_fallback() {
    assert_eq!(
      resolve_invite_token(None, Some("from-route".into())),
      InviteToken::Present("from-route".into())
    );
  }

  /// Once read, an absent token is `Missing`, never `Unread`: the card
  /// must leave its placeholder even when there is nothing to show.
  #[test]
  fn nothing_to_read_is_missing() {
    assert_eq!(resolve_invite_token(None, None), InviteToken::Missing);
    assert_eq!(
      resolve_invite_token(None, Some(String::new())),
      InviteToken::Missing
    );
  }
}
