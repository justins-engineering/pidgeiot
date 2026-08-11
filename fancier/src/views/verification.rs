use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, continue_anchor_href, extract_ui_messages, url_query_param};
use crate::{Configuration, Create};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{
  create_browser_verification_flow, get_verification_flow,
};

#[component]
pub fn VerificationFlow(flow: Option<String>) -> Element {
  // 1. Fetch or initialize the verification flow
  let get_flow = use_resource(move || {
    let flow_param = flow.clone();

    async move {
      let config = Configuration::create();

      // The address bar, not the route prop, is the source of truth for
      // `?flow=`: SSG hydration restores the prerendered `flow: None` route
      // on every full-page load — see helpers::url_query_param. Without
      // this, the post-registration redirect to `?flow=<id>` minted a NEW
      // verification flow (asking for an email address) instead of showing
      // the code-entry form for the flow Kratos just created.
      let flow_param = url_query_param("flow").or(flow_param);

      if let Some(id) = flow_param {
        match get_verification_flow(&config, &id, None).await {
          Ok(res) => {
            // Kratos v26 + `use_continue_with_transitions`: once the code is
            // accepted the flow reaches `passed_challenge`, and its UI is
            // just a success message plus a manual "Continue" anchor to the
            // after-verification return URL (/session/local?state=true).
            // Follow that transition automatically — the SPA is expected to
            // consume it, not render it — so register → verify → signed-in
            // completes with zero extra clicks. The success page still
            // renders momentarily below while the browser navigates.
            if res.state.as_ref().and_then(|s| s.as_str()) == Some("passed_challenge")
              && let Some(href) = continue_anchor_href(&res.ui)
              && let Some(window) = web_sys::window()
            {
              let _ = window.location().set_href(&href);
            }
            return Ok(res);
          }
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
            return Err(rsx! {
              div { class: "alert alert-error", "Network Error: {e:#?}" }
            });
          }
        }
      }

      match create_browser_verification_flow(&config, None).await {
        Ok(res) => Ok(res),
        Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
          Err(res.view_response_content())
        }
        Err(e) => Err(rsx! {
          div { class: "alert alert-error", "Network Error: {e:#?}" }
        }),
      }
    }
  });

  // 2. Render the UI
  match &*get_flow.read() {
    Some(Ok(res)) => {
      let error_messages = extract_ui_messages(&res.ui);

      rsx! {
        h1 { class: "text-center text-2xl mt-10", "Account Verification" }
        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            if !error_messages.is_empty() {
              div { class: "flex flex-col gap-2 mb-4",
                for (variant , msg) in error_messages {
                  Alert { variant, persistent: false, "{msg}" }
                }
              }
            }

            // Pure HTML submission.
            FormBuilder { ui: *res.ui.to_owned() }
          }
        }
      }
    }
    Some(Err(err_elem)) => rsx! {
      div { class: "mx-auto max-w-lg mt-10", {err_elem.clone()} }
    },
    None => rsx! {
      div { class: "flex justify-center mt-10",
        p { class: "animate-pulse", "Loading verification flow..." }
      }
    },
  }
}
