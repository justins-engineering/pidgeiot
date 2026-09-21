use crate::components::{Alert, FormBuilder, TermsNotice};
use crate::helpers::{
  DisplayError, extract_ui_messages, kratos_return_to, url_query_param, view_network_error,
};
use crate::{Configuration, Create, Route};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{
  create_browser_registration_flow, get_registration_flow,
};
use ory_kratos_client_wasm::models::ui_node::GroupEnum;

/// Whether this registration step is the one that creates the account.
///
/// Kratos's default registration takes the profile traits first and asks
/// for a credential on a second step, with a full page load between them
/// that resets every signal, so a tick on the first step is gone by the
/// second. The credential step is the one carrying nodes outside the
/// default and profile groups; on a one-step flow that is the only step.
fn creates_the_identity(groups: impl Iterator<Item = GroupEnum>) -> bool {
  groups
    .into_iter()
    .any(|group| !matches!(group, GroupEnum::Default | GroupEnum::Profile))
}

#[component]
pub fn RegisterFlow(flow: Option<String>) -> Element {
  // Hoisted above the match below, which renders the form in only one of
  // its arms: a hook called from one arm would shift this scope's hook
  // indices as the flow resolves.
  let terms_ok = use_signal(|| false);

  // 1. Fetch or initialize the flow natively
  let get_flow = use_resource(move || {
    let flow_param = flow.clone();

    async move {
      let config = Configuration::create();

      // The address bar, not the route prop, is the source of truth for
      // `?flow=`: SSG hydration restores the prerendered `flow: None` route
      // on every full-page load — see helpers::url_query_param. Without
      // this, Kratos's 303 back to `?flow=<id>` after each form POST loses
      // its id here, so the SPA mints a brand-new flow and re-renders a
      // fresh empty form with no error, every time.
      let flow_param = url_query_param("flow").or(flow_param);

      if let Some(id) = flow_param {
        match get_registration_flow(&config, &id, None).await {
          Ok(res) => return Ok(res),
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res))
            if res.status == 410 || res.status == 404 || res.status == 403 =>
          {
            // Expired (410), unknown (404), or another browser's (403) flow
            // id: fall through and mint a fresh flow inline. Don't
            // nav.replace(flow: None) and rely on a fresh fetch instead —
            // use_resource's future does not rerun on the post-replace
            // rerender, so that approach hangs forever.
          }
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
            return Err(res.view_response_content());
          }
          Err(e) => {
            return Err(view_network_error(&e));
          }
        }
      }

      // The same hand-back for the verification step Kratos runs after
      // registration, so that step ends on this host too.
      let return_to = kratos_return_to(true);
      match create_browser_registration_flow(
        &config,
        return_to.as_deref(),
        None,
        return_to.as_deref(),
        None,
        None,
      )
      .await
      {
        Ok(res) => Ok(res),
        Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
          Err(res.view_response_content())
        }
        Err(e) => Err(view_network_error(&e)),
      }
    }
  });

  // 2. Render the UI
  match &*get_flow.read() {
    Some(Ok(res)) => {
      let error_messages = extract_ui_messages(&res.ui);
      let credential_step = creates_the_identity(res.ui.nodes.iter().map(|node| node.group));

      rsx! {
        h1 { class: "text-center text-2xl mt-10", "Sign Up" }
        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            if !error_messages.is_empty() {
              div { class: "flex flex-col gap-2 mb-4",
                for (variant , msg) in error_messages {
                  Alert { variant, persistent: false, "{msg}" }
                }
              }
            }

            // Notice on every step, the box only on the one that creates
            // the account. Asking twice for the same act reads as a bug,
            // and the earlier step binds nobody to anything.
            div { class: "mb-6",
              TermsNotice { accepted: credential_step.then_some(terms_ok) }
            }

            // Pure HTML submission. `inert` is the whole enforcement here,
            // and it is allowed to be only a browser behaviour: an account
            // created without the tick meets the gate on its first
            // dashboard entry, which is where the record is written.
            div {
              "inert": (credential_step && !terms_ok()).then_some(""),
              class: if credential_step && !terms_ok() { "opacity-60" } else { "" },
              FormBuilder { ui: *res.ui.to_owned() }
            }
            p { class: "text-sm leading-6 mt-4",
              "Already have an account? "
              Link {
                to: Route::LoginFlow { flow: None },
                class: "link-primary link-hover",
                "Login →"
              }
            }
          }
        }
      }
    }
    Some(Err(err_elem)) => rsx! {
      div { class: "mx-auto max-w-lg mt-10", {err_elem.clone()} }
    },
    None => rsx! {
      div { class: "flex justify-center mt-10",
        p { class: "animate-pulse", "Loading registration flow..." }
      }
    },
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn only_the_step_that_asks_for_a_credential_creates_the_identity() {
    let profile_step = [GroupEnum::Default, GroupEnum::Default, GroupEnum::Profile];
    assert!(!creates_the_identity(profile_step.into_iter()));

    let password_step = [GroupEnum::Default, GroupEnum::Password, GroupEnum::Profile];
    assert!(creates_the_identity(password_step.into_iter()));

    // A passkey-only deployment asks for its credential on that step too.
    let passkey_step = [GroupEnum::Default, GroupEnum::Passkey];
    assert!(creates_the_identity(passkey_step.into_iter()));
  }
}
