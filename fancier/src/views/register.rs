use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, extract_ui_messages};
use crate::{Configuration, Create, Route};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{
  create_browser_registration_flow, get_registration_flow,
};

#[component]
pub fn RegisterFlow(flow: Option<String>) -> Element {
  let nav = use_navigator();

  // 1. Fetch or initialize the flow natively
  let get_flow = use_resource(move || {
    let flow_param = flow.clone();
    let nav = nav;

    async move {
      let config = Configuration::create();

      match flow_param {
        Some(id) => match get_registration_flow(&config, &id, None).await {
          Ok(res) => Ok(res),
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) if res.status == 410 => {
            nav.replace(Route::RegisterFlow { flow: None });
            Err(rsx! {
              div { class: "animate-pulse", "Refreshing expired session..." }
            })
          }
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
            Err(res.view_response_content())
          }
          Err(e) => Err(rsx! {
            div { class: "alert alert-error", "Network Error: {e:#?}" }
          }),
        },
        None => match create_browser_registration_flow(&config, None, None, None, None, None).await
        {
          Ok(res) => Ok(res),
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
            Err(res.view_response_content())
          }
          Err(e) => Err(rsx! {
            div { class: "alert alert-error", "Network Error: {e:#?}" }
          }),
        },
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
