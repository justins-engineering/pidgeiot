use crate::components::{DisplayError, FormBuilder};
use crate::{Configuration, Create, Route};
use dioxus::logger::tracing::{debug, error};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{create_browser_recovery_flow, get_recovery_flow};

#[component]
pub fn AccountRecovery() -> Element {
  let create_flow = use_resource(move || async move {
    create_browser_recovery_flow(&Configuration::create(), None).await
  });

  return match &*create_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        debug!("{res:#?}");
        rsx! {
          h1 { class: "text-center text-2xl", "Account Recovery" }
          div { class: "mx-auto w-full max-w-lg",
            FormBuilder { ui: *res.ui.to_owned() }
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
pub fn RecoveryFlow(flow: String) -> Element {
  let get_flow = use_resource(move || {
    let id = flow.to_owned();
    async move { get_recovery_flow(&Configuration::create(), &id, None).await }
  });

  return match &*get_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        debug!("{res:#?}");
        rsx! {
          h1 { class: "text-center text-2xl", "Account Recovery" }
          div { class: "mx-auto w-full max-w-lg",
            FormBuilder { ui: *res.ui.to_owned() }
          }
        }
      }
      Err(err) => {
        navigator().replace(Route::SignUp {});
        rsx! {
          p { "Failed to get RecoveryFlow! Error: {err:?}" }
        }
      }
    },
    None => rsx! {},
  };
}
