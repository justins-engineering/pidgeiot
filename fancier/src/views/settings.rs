use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, extract_ui_messages};
use crate::{AuthState, Configuration, Create, Route, Session};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use futures_util::StreamExt;
use ory_kratos_client_wasm::apis::frontend_api::{create_browser_settings_flow, get_settings_flow};
use ory_kratos_client_wasm::apis::urlencode;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

#[component]
pub fn SettingsFlow(flow: Option<String>) -> Element {
  // 1. Change the error arm of the Result to `Element` to support polymorphic error rendering
  let mut flow_state =
    use_signal(|| None::<Result<ory_kratos_client_wasm::models::SettingsFlow, Element>>);

  let mut session = use_context::<Session>();
  let nav = use_navigator();

  // 2. Initialize or fetch the settings flow
  use_future(move || {
    let flow_param = flow.clone();

    async move {
      let config = Configuration::create();

      // Map standard Kratos ResponseErrors through the DisplayError trait,
      // and fallback to raw text blocks for network/parsing failures.
      let result = match flow_param {
        Some(id) => match get_settings_flow(&config, &id, None, None).await {
          Ok(res) => Ok(res),
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
            Err(res.view_response_content())
          }
          Err(e) => Err(rsx! {
            div { class: "alert alert-error", "Network Error: {e:#?}" }
          }),
        },
        None => match create_browser_settings_flow(&config, None, None).await {
          Ok(res) => Ok(res),
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
            Err(res.view_response_content())
          }
          Err(e) => Err(rsx! {
            div { class: "alert alert-error", "Network Error: {e:#?}" }
          }),
        },
      };

      flow_state.set(Some(result));
    }
  });

  // 3. Handle the native async form submission via web-sys
  let submit_form = use_coroutine(
    move |mut rx: UnboundedReceiver<(String, std::collections::HashMap<String, String>)>| async move {
      while let Some((action_url, form_data)) = rx.next().await {
        let mut form_encoded = String::new();
        for (i, (key, value)) in form_data.iter().enumerate() {
          if i > 0 {
            form_encoded.push('&');
          }
          form_encoded.push_str(&urlencode(key));
          form_encoded.push('=');
          form_encoded.push_str(&urlencode(value));
        }

        let client = web_sys::RequestInit::new();
        client.set_method("POST");
        client.set_mode(web_sys::RequestMode::Cors);
        client.set_credentials(web_sys::RequestCredentials::Include);
        client.set_body(&wasm_bindgen::JsValue::from_str(&form_encoded));

        let req_builder = match web_sys::Request::new_with_str_and_init(&action_url, &client) {
          Ok(req) => req,
          Err(e) => {
            flow_state.set(Some(Err(rsx! {
              div { class: "alert alert-error", "Failed to build request: {e:?}" }
            })));
            continue;
          }
        };

        let _ = req_builder.headers().set("Accept", "application/json");
        let _ = req_builder
          .headers()
          .set("Content-Type", "application/x-www-form-urlencoded");

        let window = web_sys::window().expect("Failed to get Window object");

        match JsFuture::from(window.fetch_with_request(&req_builder)).await {
          Ok(resp_value) => {
            let resp: web_sys::Response =
              resp_value.dyn_into().expect("Failed to cast to Response");
            let status = resp.status();

            if (200..300).contains(&status) {
              session.state.set(AuthState::Authenticated);
              nav.replace(Route::Dashboard {});
            } else if status == 400 {
              match JsFuture::from(resp.json().unwrap()).await {
                Ok(json_val) => {
                  match serde_wasm_bindgen::from_value::<ory_kratos_client_wasm::models::SettingsFlow>(
                    json_val,
                  ) {
                    Ok(updated_flow) => flow_state.set(Some(Ok(updated_flow))),
                    Err(e) => flow_state.set(Some(Err(rsx! {
                      div { class: "alert alert-error",
                        "Failed to deserialize Kratos error JSON: {e}"
                      }
                    }))),
                  }
                }
                Err(e) => flow_state.set(Some(Err(rsx! {
                  div { class: "alert alert-error",
                    "Failed to read JSON response: {e:?}"
                  }
                }))),
              }
            } else {
              match JsFuture::from(resp.text().unwrap()).await {
                Ok(text_val) => {
                  let err_text = text_val.as_string().unwrap_or_default();
                  flow_state.set(Some(Err(rsx! {
                    div { class: "alert alert-error", "HTTP {status}: {err_text}" }
                  })));
                }
                Err(_) => flow_state.set(Some(Err(rsx! {
                  div { class: "alert alert-error", "Unhandled HTTP {status}" }
                }))),
              }
            }
          }
          Err(e) => {
            error!("Network error during submission: {:?}", e);
            flow_state.set(Some(Err(rsx! {
              div { class: "alert alert-error", "Network Error: {e:?}" }
            })));
          }
        }
      }
    },
  );

  // 4. Render the UI
  match &*flow_state.read() {
    Some(Ok(res)) => {
      let error_messages = extract_ui_messages(&res.ui);
      let action_url = res.ui.action.clone();

      // --- THE SPA FIX ---
      // Dynamically extract the primary submit button's strategy from the Kratos schema.
      // We fallback to `method=password` just in case the schema is malformed.
      let submit_strategy = res
        .ui
        .nodes
        .iter()
        .find_map(|node| {
          if let ory_kratos_client_wasm::models::UiNodeAttributes::Input(i) = &*node.attributes
            && i.r#type
              == ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Submit
            && let Some(Some(serde_json::Value::String(s))) = &i.value
          {
            return Some((i.name.clone(), s.clone()));
          }
          None
        })
        .unwrap_or_else(|| ("method".to_string(), "code".to_string()));

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

            FormBuilder {
              ui: *res.ui.to_owned(),
              on_submit: move |ev: Event<FormData>| {
                  let mut parsed_data = std::collections::HashMap::new();

                  for (key, value) in ev.values().iter() {
                      if let dioxus::events::FormValue::Text(val) = value {
                          parsed_data.insert(key.clone(), val.clone());
                      }
                  }

                  // INJECT THE MISSING BUTTON STRATEGY
                  if !parsed_data.contains_key(&submit_strategy.0) {
                      parsed_data.insert(submit_strategy.0.clone(), submit_strategy.1.clone());
                  }

                  submit_form.send((action_url.clone(), parsed_data));
              },
            }
          }
        }
      }
    }
    Some(Err(err_elem)) => {
      // Render the DisplayError nodes directly into the tree
      rsx! {
        div { class: "mx-auto max-w-lg mt-10", {err_elem.clone()} }
      }
    }
    None => {
      rsx! {
        div { class: "flex justify-center mt-10",
          p { class: "animate-pulse", "Loading settings flow..." }
        }
      }
    }
  }
}
