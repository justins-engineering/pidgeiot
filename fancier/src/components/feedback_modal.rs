use crate::{Configuration, Create, Session, api};
use capsules::{FeedbackCategory, FeedbackRequest, MAX_FEEDBACK_MESSAGE_BYTES};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdCircleCheck, LdX};
use ory_kratos_client_wasm::apis::frontend_api::to_session;

/// Context handle for opening the feedback modal from anywhere in the app
/// chrome -- provided by `Wrapper` (which owns the conditional render),
/// consumed by `Footer`'s "Send Feedback" link and `Navbar`'s menu items.
/// A bare `Signal<bool>` in context would be ambiguous the moment a second
/// boolean signal wanted the same treatment, hence the newtype.
#[derive(Clone, Copy)]
pub struct FeedbackForm(pub Signal<bool>);

/// The feedback form (`POST /feedback` -- see docs/api.md's
/// "Feedback" section). Rendered conditionally by `Wrapper` from the
/// `FeedbackForm` context signal rather than toggled via a native
/// `<dialog>`, same convention as `TokenReveal`/`DeletePigeonModal`: the
/// form holds reset-sensitive state (typed message, sent/error status), so
/// each open must remount it fresh instead of resurfacing a stale draft or
/// a leftover "Feedback sent" confirmation.
///
/// Works logged-out too -- the route requires no session (marketing pages
/// link the same chrome). When a session *is* present, the contact-email
/// field is prefilled from the Kratos identity's own `traits.email`
/// (fetched via `to_session`, the same whoami call `SetSessionCookie`
/// already makes) -- prefill only, still editable/clearable, and the
/// backend independently records the authenticated identity server-side
/// regardless of what this field says.
///
/// `problem: true` is the floating "Report a problem" flow: same form,
/// category preset to Problem, and the breadcrumb trail + build hash
/// attached automatically (with a line saying so) -- what lets "it broke
/// when I clicked save" be debugged without asking for a reproduction.
#[component]
pub fn FeedbackModal(on_close: EventHandler<()>, #[props(default)] problem: bool) -> Element {
  let is_logged_in = use_context::<Session>().state.read().is_authenticated();
  let mut category = use_signal(move || {
    if problem {
      "problem".to_string()
    } else {
      "general".to_string()
    }
  });
  let mut message = use_signal(String::new);
  let mut contact_email = use_signal(String::new);
  let mut is_sending = use_signal(|| false);
  let mut sent = use_signal(|| false);
  let mut error_msg = use_signal(|| Option::<String>::None);

  use_future(move || async move {
    if !is_logged_in {
      return;
    }
    let config = Configuration::create();
    let Ok(session) = to_session(&config, None, None, None).await else {
      return;
    };
    let Some(identity) = session.identity else {
      return;
    };
    let email = identity
      .traits
      .as_ref()
      .and_then(|traits| traits.get("email"))
      .and_then(|v| v.as_str())
      .map(|s| s.to_string());
    // Only prefill an untouched field -- never clobber something the user
    // already started typing while the whoami round trip was in flight.
    if let Some(email) = email
      && contact_email.read().is_empty()
    {
      contact_email.set(email);
    }
  });

  let can_submit = !message.read().trim().is_empty() && !is_sending();

  let submit = move |_| async move {
    is_sending.set(true);
    error_msg.set(None);

    let selected_category = match category.read().as_str() {
      "bug" => FeedbackCategory::Bug,
      "feature_request" => FeedbackCategory::FeatureRequest,
      "problem" => FeedbackCategory::Problem,
      _ => FeedbackCategory::General,
    };
    let contact = contact_email.read().trim().to_string();
    let page_context = web_sys::window().and_then(|w| w.location().pathname().ok());

    // Attached for problem reports only, and disclosed in the form:
    // shape-only request lines plus the build hash, never bodies.
    let diagnostics = if selected_category == FeedbackCategory::Problem {
      crate::helpers::error_report::diagnostics_string()
    } else {
      None
    };

    let req = FeedbackRequest {
      message: message(),
      category: Some(selected_category),
      contact_email: (!contact.is_empty()).then_some(contact),
      page_context,
      diagnostics,
    };

    match api::feedback::send(&req).await {
      Ok(()) => sent.set(true),
      Err(e) => error_msg.set(Some(e)),
    }
    is_sending.set(false);
  };

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "feedback_modal_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape && !is_sending() {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-md",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          disabled: is_sending(),
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        if sent() {
          // Success state replaces the form entirely -- the cleared form is
          // guaranteed by the remount-on-next-open convention above, so
          // nothing here needs to reset signals by hand.
          div { class: "flex flex-col items-center text-center py-6 gap-3",
            Icon {
              icon: LdCircleCheck,
              class: "size-12 text-success",
              title: "Success",
            }
            h3 { class: "text-lg font-bold", id: "feedback_modal_title", "Feedback sent" }
            p { class: "text-sm text-base-content/80",
              "Thanks for helping make PidgeIoT better. If you left a contact email, we may follow up."
            }
            button {
              class: "btn btn-primary mt-2",
              onclick: move |_| on_close.call(()),
              "Close"
            }
          }
        } else {
          h3 { class: "text-lg font-bold", id: "feedback_modal_title",
            if problem {
              "Report a problem"
            } else {
              "Send Feedback"
            }
          }
          p { class: "py-2 text-sm text-base-content/80",
            if problem {
              "Something not working? Say what you were doing and what happened -- it goes straight to the team."
            } else {
              "Spotted a bug, missing a feature, or just have a thought? It goes straight to the team."
            }
          }
          fieldset { class: "fieldset",
            label { class: "fieldset-legend text-xs font-semibold", r#for: "feedback_category",
              "Category"
            }
            select {
              class: "select select-bordered w-full",
              id: "feedback_category",
              disabled: is_sending(),
              value: "{category}",
              onchange: move |e| category.set(e.value()),
              option { value: "problem", selected: problem, "Problem report" }
              option { value: "bug", "Bug report" }
              option { value: "feature_request", "Feature request" }
              option { value: "general", selected: !problem, "General feedback" }
            }
            label { class: "fieldset-legend text-xs font-semibold", r#for: "feedback_message",
              "Message"
            }
            textarea {
              class: "textarea textarea-bordered w-full h-32",
              id: "feedback_message",
              placeholder: "What's on your mind?",
              // Chars, not bytes -- multibyte input can still exceed the
              // server's byte cap, which then answers 413 with its own
              // "too long" copy, so this is a convenience rail, not the
              // enforcement point.
              maxlength: MAX_FEEDBACK_MESSAGE_BYTES as i64,
              disabled: is_sending(),
              value: "{message}",
              oninput: move |e| message.set(e.value()),
              onmounted: move |e| async move {
                  let _ = e.set_focus(true).await;
              },
            }
            label { class: "fieldset-legend text-xs font-semibold", r#for: "feedback_contact",
              "Contact email "
              span { class: "font-normal opacity-60", "(optional, if you'd like a reply)" }
            }
            input {
              class: "input input-bordered w-full",
              id: "feedback_contact",
              r#type: "email",
              autocomplete: "email",
              placeholder: "you@example.com",
              disabled: is_sending(),
              value: "{contact_email}",
              oninput: move |e| contact_email.set(e.value()),
            }
          }
          if category() == "problem" {
            // Disclosed, not silent: the same shape-only trail an automatic
            // error report carries.
            p { class: "text-xs text-base-content/60 pt-2",
              "Your recent in-app requests (method, page, status -- never contents) and the app build are attached automatically."
            }
          }
          if let Some(err) = error_msg.read().as_ref() {
            div { class: "alert alert-error mt-3 text-sm", role: "alert", "{err}" }
          }
          div { class: "modal-action",
            button {
              class: "btn btn-ghost",
              disabled: is_sending(),
              onclick: move |_| on_close.call(()),
              "Cancel"
            }
            button {
              class: "btn btn-primary",
              disabled: !can_submit,
              onclick: submit,
              if is_sending() {
                span { class: "loading loading-spinner loading-sm" }
              } else if problem {
                "Send report"
              } else {
                "Send Feedback"
              }
            }
          }
        }
      }
    }
  }
}
