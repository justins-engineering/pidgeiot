use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, extract_ui_messages, url_query_param};
use crate::{Configuration, Create, Route};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{
  create_browser_registration_flow, get_registration_flow,
};

#[component]
pub fn RegisterFlow(flow: Option<String>) -> Element {
  // 1. Fetch or initialize the flow natively
  let get_flow = use_resource(move || {
    let flow_param = flow.clone();

    async move {
      let config = Configuration::create();

      // The address bar, not the route prop, is the source of truth for
      // `?flow=`: SSG hydration restores the prerendered `flow: None` route
      // on every full-page load — see helpers::url_query_param. This was the
      // prod signup outage: Kratos's 303 back to `?flow=<id>` after each
      // form POST lost its id here, so the SPA minted a brand-new flow and
      // re-rendered a fresh empty form with no error, every time.
      let flow_param = url_query_param("flow").or(flow_param);

      if let Some(id) = flow_param {
        match get_registration_flow(&config, &id, None).await {
          Ok(res) => return Ok(res),
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res))
            if res.status == 410 || res.status == 404 || res.status == 403 =>
          {
            // Expired (410), unknown (404), or another browser's (403) flow
            // id: fall through and mint a fresh flow. The previous
            // nav.replace(flow: None) + "Refreshing expired session..."
            // placeholder could never recover — use_resource's future does
            // not rerun on the post-replace rerender — so it hung forever.
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

      match create_browser_registration_flow(&config, None, None, None, None, None).await {
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

            // Pure HTML submission.
            FormBuilder { ui: *res.ui.to_owned() }
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
