use crate::components::{DisplayError, FormBuilder};
use crate::{Configuration, Create, Route};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{
  create_browser_registration_flow, get_registration_flow,
};

#[component]
pub fn SignUp() -> Element {
  let create_flow = use_resource(move || async move {
    create_browser_registration_flow(&Configuration::create(), None, None, None, None, None).await
  });

  return match &*create_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        rsx! {
          h1 { class: "text-center text-2xl mt-10", "Sign Up" }
          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              FormBuilder { ui: *res.ui.to_owned() }
              p { class: "text-sm leading-6",
                "Already have an account? "
                Link {
                  to: Route::SignIn {},
                  class: "link-primary link-hover",
                  "Login →"
                }
              }
            }
          }
        }
      }
      Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => rsx! {
        {res.to_owned().view_response_content()}
      },

      Err(err) => {
        error!("{err:#?}");
        rsx! {
          p { "Failed to get RegistrationFlow! Error:" }
          p { "{err:#?}" }
        }
      }
    },
    None => rsx! {},
  };
}

#[component]
pub fn RegisterFlow(flow: String) -> Element {
  let get_flow = use_resource(move || {
    let id = flow.to_owned();
    async move { get_registration_flow(&Configuration::create(), &id, None).await }
  });

  return match &*get_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        rsx! {
          h1 { class: "text-center text-2xl mt-10", "Sign Up" }
          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              FormBuilder { ui: *res.ui.to_owned() }
              p { class: "text-sm leading-6",
                "Already have an account? "
                Link {
                  to: Route::SignIn {},
                  class: "link-primary link-hover",
                  "Login →"
                }
              }
            }
          }
        }
      }
      Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
        error!("ResponseError");
        rsx! {
          {res.to_owned().view_response_content()}
        }
      }
      Err(err) => {
        // navigator().replace(Route::SignUp {});
        error!("{err:#?}");
        rsx! {
          p { "Failed to get RegistrationFlow! Error:\n {err:#?}" }
        }
      }
    },
    None => rsx! {},
  };
}
