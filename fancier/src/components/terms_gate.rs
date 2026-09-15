//! Notice of the published legal documents: the box that accepts them, the
//! panel that holds the dashboard until it is ticked, and the line that
//! names them where a subscription is bought.
//!
//! One component carries the notice because the registration form and the
//! gate ask the same thing: the words a person ticks and the row we keep
//! have to describe the same act.
//!
//! The record is written by `POST /account/terms` against an authenticated
//! session, so the version, the time, the address and the user agent are
//! all the server's. Nothing here supplies any of them.

use crate::components::OryLogOut;
use crate::{Route, api};
use capsules::TERMS_VERSION;
use capsules::consent::{TERMS_ASSENT_CHECKOUT_NOTICE, TERMS_ASSENT_LABEL, TermsAssentStatus};
use dioxus::prelude::*;

/// Whether the dashboard is held behind the gate.
///
/// A status we could not read renders the app: an unreadable status is not
/// evidence that assent is missing, and a database blip must never lock an
/// account out of its own fleet. The next sign-in asks again. A status not
/// read *yet* is a different state and `AuthGuard` holds it separately;
/// this answer is only ever asked for a read that finished.
pub fn blocks_dashboard(assent: Option<&TermsAssentStatus>) -> bool {
  assent.is_some_and(|status| !status.is_current())
}

/// Whether this dashboard's own copy of the Terms is the version the API
/// would stamp a row with.
///
/// False for the minutes of a version deploy: fancier ships first, so the
/// pages carry the new text while dovecote still calls the old one
/// current. Accepting in that window would write a row naming a version
/// the person was not shown, which is the one thing the deploy order
/// exists to prevent.
pub fn versions_agree(status: &TermsAssentStatus) -> bool {
  status.current_version == TERMS_VERSION
}

/// The four published documents, and the box that accepts them when a
/// signal is given. Without one it is notice alone, which is what the
/// registration step that creates no account shows.
#[component]
pub fn TermsNotice(accepted: Option<Signal<bool>>) -> Element {
  rsx! {
    div { class: "rounded-box border border-base-300 bg-base-200/40 p-4",
      p { class: "text-sm text-base-content/70",
        "Please read the "
        Link { class: "link link-secondary", to: Route::TermsPage {}, "Terms of Service" }
        ", the "
        Link { class: "link link-secondary", to: Route::PrivacyPage {}, "Privacy Policy" }
        ", the "
        Link { class: "link link-secondary", to: Route::DpaPage {}, "Data Processing Agreement" }
        " and the "
        Link { class: "link link-secondary", to: Route::SubprocessorsPage {}, "sub-processor list" }
        "."
      }
      // Plain utilities rather than daisyUI's `label`, whose `white-space:
      // nowrap` runs this sentence off the side of a phone.
      if let Some(mut accepted) = accepted {
        label { class: "mt-3 flex cursor-pointer items-start gap-3",
          input {
            r#type: "checkbox",
            class: "checkbox checkbox-sm mt-0.5 shrink-0",
            checked: accepted(),
            onchange: move |evt: Event<FormData>| accepted.set(evt.checked()),
          }
          span { class: "text-sm", "{TERMS_ASSENT_LABEL}" }
        }
      }
    }
  }
}

/// The line beside a purchase button. Subscribing is itself the act, and
/// the gate already holds the tick, so this names what is being accepted
/// rather than asking for it a second time.
#[component]
pub fn PurchaseTermsNotice() -> Element {
  rsx! {
    p { class: "text-xs text-base-content/60",
      "{TERMS_ASSENT_CHECKOUT_NOTICE[0]}"
      Link { class: "link link-secondary", to: Route::TermsPage {}, "Terms of Service" }
      "{TERMS_ASSENT_CHECKOUT_NOTICE[1]}{TERMS_VERSION}{TERMS_ASSENT_CHECKOUT_NOTICE[2]}"
      Link { class: "link link-secondary", to: Route::DpaPage {}, "Data Processing Agreement" }
      "{TERMS_ASSENT_CHECKOUT_NOTICE[3]}"
    }
  }
}

/// The full-screen panel an account meets when it has not accepted the
/// published Terms. Not dismissable and not a modal: the dashboard behind
/// it is what the record is kept for. The legal pages are public routes,
/// so every link on it is reachable from here, and signing out works.
#[component]
pub fn TermsGate(assent: Signal<Option<TermsAssentStatus>>) -> Element {
  let accepted = use_signal(|| false);
  let mut busy = use_signal(|| false);
  let mut error = use_signal(|| Option::<String>::None);

  let deploying = assent
    .read()
    .as_ref()
    .is_some_and(|status| !versions_agree(status));
  if deploying {
    return rsx! {
      section { id: "terms-gate", class: "px-4 py-16",
        div { class: "mx-auto max-w-xl rounded-2xl border border-base-300 bg-base-100 p-6 md:p-8",
          h1 { class: "text-2xl font-bold tracking-tight", "The Terms are being published" }
          p { class: "mt-3 text-base-content/70",
            "A new version is going out right now, and this page is not yet showing the one
             your acceptance would be recorded against. Reload in a minute."
          }
          div { class: "mt-6",
            OryLogOut {}
          }
        }
      }
    };
  }

  // Both from the status rather than this build's constant: the version
  // named on screen is then the version the row will carry, and the person
  // can see whether they are being asked for the first time or again.
  let version = assent
    .read()
    .as_ref()
    .map_or_else(|| TERMS_VERSION.to_string(), |s| s.current_version.clone());
  let previous = assent
    .read()
    .as_ref()
    .and_then(|s| s.accepted_version.clone());

  rsx! {
    section { id: "terms-gate", class: "px-4 py-16",
      div { class: "mx-auto max-w-xl rounded-2xl border border-base-300 bg-base-100 p-6 md:p-8",
        h1 { class: "text-2xl font-bold tracking-tight", "Please accept the Terms of Service" }
        p { class: "mt-3 text-base-content/70",
          "These are the terms your account runs under, dated "
          span { class: "font-semibold", "{version}" }
          ". We ask once for each published version, so you will not see this again until they
           change."
        }
        if let Some(previous) = previous {
          p { class: "mt-2 text-sm text-base-content/60",
            "They replace the version dated {previous}, which you accepted earlier."
          }
        }
        div { class: "mt-6",
          TermsNotice { accepted }
        }
        if let Some(msg) = error() {
          p { class: "mt-4 text-sm text-error", "{msg}" }
        }
        div { class: "mt-6 flex flex-wrap items-center gap-3",
          button {
            class: "btn btn-primary font-bold",
            disabled: !accepted() || busy(),
            onclick: move |_| async move {
                busy.set(true);
                error.set(None);
                match api::terms::accept().await {
                    Some(status) => assent.set(Some(status)),
                    None => {
                        error
                            .set(
                                Some(
                                    "We could not record that. Please try again in a moment."
                                        .to_string(),
                                ),
                            )
                    }
                }
                busy.set(false);
            },
            if busy() {
              span { class: "loading loading-spinner loading-sm" }
            } else {
              "Agree and continue"
            }
          }
          OryLogOut {}
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn status(accepted: Option<&str>) -> TermsAssentStatus {
    TermsAssentStatus {
      current_version: "2026-09-14".to_string(),
      accepted_version: accepted.map(str::to_string),
      accepted_at: None,
    }
  }

  #[test]
  fn an_unreadable_status_renders_the_app() {
    assert!(!blocks_dashboard(None));
  }

  #[test]
  fn the_panel_asks_only_for_the_version_this_build_publishes() {
    assert!(versions_agree(&TermsAssentStatus {
      current_version: TERMS_VERSION.to_string(),
      accepted_version: None,
      accepted_at: None,
    }));
    // The API considers another version current: this build is one side of
    // a deploy and its pages cannot be what a row would name.
    assert!(!versions_agree(&TermsAssentStatus {
      current_version: "2099-01-01".to_string(),
      accepted_version: None,
      accepted_at: None,
    }));
  }

  #[test]
  fn only_a_version_other_than_the_published_one_blocks() {
    assert!(!blocks_dashboard(Some(&status(Some("2026-09-14")))));
    assert!(blocks_dashboard(Some(&status(Some("2026-09-04")))));
    assert!(blocks_dashboard(Some(&status(None))));
  }
}
