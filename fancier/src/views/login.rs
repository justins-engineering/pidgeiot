use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, extract_ui_messages, url_query_param};
use crate::models::AlertVariant;
use crate::{Configuration, Create, Route, Session};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{create_browser_login_flow, get_login_flow};

#[component]
pub fn LoginFlow(flow: Option<String>) -> Element {
  // Someone sent here by their session lapsing gets told that, rather than
  // being dropped on a login form with no explanation for why the page
  // they asked for went away. Read outside the flow-fetch match so the
  // notice is up while the flow is still loading, not just after.
  let session = use_context::<Session>();
  let signed_out = (session.signed_out)();

  // 1. Fetch or initialize the flow natively
  let get_flow = use_resource(move || {
    let flow_param = flow.clone();

    async move {
      let config = Configuration::create();

      // The address bar, not the route prop, is the source of truth for
      // `?flow=`: SSG hydration restores the prerendered `flow: None` route
      // on every full-page load — see helpers::url_query_param.
      let flow_param = url_query_param("flow").or(flow_param);

      if let Some(id) = flow_param {
        match get_login_flow(&config, &id, None).await {
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
            return Err(rsx! {
              div { class: "alert alert-error", "Network Error: {e:#?}" }
            });
          }
        }
      }

      match create_browser_login_flow(&config, None, None, None, None, None, None, None, None).await
      {
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
  rsx! {
    if signed_out {
      div { class: "mx-auto w-full max-w-lg mt-10",
        Alert { variant: AlertVariant::Warning,
          "Your session ended, so you were signed out. Sign in again to pick up where you left off."
        }
      }
    }
    match &*get_flow.read() {
      Some(Ok(res)) => {
        let error_messages = extract_ui_messages(&res.ui);

        rsx! {
          h1 { class: "text-center text-2xl mt-10", "Sign In" }
          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              if !error_messages.is_empty() {
                div { class: "flex flex-col gap-2 mb-4",
                  for (variant , msg) in error_messages {
                    Alert { variant, persistent: false, "{msg}" }
                  }
                }
              }

              // Pure HTML submission. Browser handles the POST, strategy injection, and 303 Redirect.
              FormBuilder { ui: *res.ui.to_owned() }
              p { class: "text-sm leading-6 mt-4",
                "Don't have an account? "
                Link {
                  to: Route::RegisterFlow { flow: None },
                  class: "link-primary link-hover",
                  "Register →"
                }
              }
              // The recovery flow is reachable only from here; someone who
              // cannot sign in has no other way to find it.
              p { class: "text-sm leading-6",
                "Forgot your password? "
                Link {
                  to: Route::RecoveryFlow { flow: None },
                  class: "link-primary link-hover",
                  "Recover your account →"
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
          p { class: "animate-pulse", "Loading login flow..." }
        }
      },
    }
  }
}
