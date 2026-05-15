use crate::components::{DisplayError, FormBuilder};
use crate::{Configuration, Create, Route};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::{create_browser_settings_flow, get_settings_flow};

#[component]
pub fn Settings() -> Element {
  let create_flow = use_resource(move || async move {
    create_browser_settings_flow(&Configuration::create(), None, None).await
  });

  return match &*create_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        rsx! {
          h1 { class: "text-center text-2xl", "User Settings" }
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
          p { "Failed to get SeetingsFlow! Error:" }
          p { "{err:#?}" }
        }
      }
    },
    None => rsx! {},
  };
}

#[component]
pub fn SettingsFlow(flow: String) -> Element {
  let get_flow = use_resource(move || {
    let id = flow.to_owned();
    async move { get_settings_flow(&Configuration::create(), &id, None, None).await }
  });

  return match &*get_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        rsx! {
          h1 { class: "text-center text-2xl", "User Settings" }
          div { class: "mx-auto w-full max-w-lg",
            FormBuilder { ui: *res.ui.to_owned() }
          }
        }
      }
      Err(err) => {
        navigator().replace(Route::SessionInfo {});
        rsx! {
          p { "Failed to get RegistrationFlow! Error: {err:?}" }
        }
      }
    },
    None => rsx! {},
  };
}
