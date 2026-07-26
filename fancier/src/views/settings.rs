use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, extract_ui_messages, url_query_param};
use crate::{Configuration, Create};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{create_browser_settings_flow, get_settings_flow};

#[component]
pub fn SettingsFlow(flow: Option<String>) -> Element {
  // 1. Fetch or initialize the settings flow
  let get_flow = use_resource(move || {
    let flow_param = flow.clone();

    async move {
      let config = Configuration::create();

      // The address bar, not the route prop, is the source of truth for
      // `?flow=`: SSG hydration restores the prerendered `flow: None` route
      // on every full-page load — see helpers::url_query_param.
      let flow_param = url_query_param("flow").or(flow_param);

      if let Some(id) = flow_param {
        match get_settings_flow(&config, &id, None, None).await {
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

      match create_browser_settings_flow(&config, None, None).await {
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
        h1 { class: "text-center text-2xl mt-10", "User Settings" }
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
        p { class: "animate-pulse", "Loading settings flow..." }
      }
    },
  }
}
