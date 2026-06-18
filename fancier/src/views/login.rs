use crate::components::{Alert, FormBuilder};
use crate::helpers::{DisplayError, extract_expired_message, extract_ui_messages};
use crate::models::AlertVariant;
use crate::{AuthState, Configuration, Create, Route, Session};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use futures_util::StreamExt;
use ory_kratos_client_wasm::apis::frontend_api::{create_browser_login_flow, get_login_flow};
use ory_kratos_client_wasm::apis::urlencode;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

#[component]
pub fn LoginFlow(flow: Option<String>) -> Element {
  let mut flow_state =
    use_signal(|| None::<Result<ory_kratos_client_wasm::models::LoginFlow, Element>>);
  let mut expiration_error = use_signal(|| None::<String>);

  let mut session = use_context::<Session>();
  let nav = use_navigator();
  let nav_for_future = nav;

  // 1. Initialize or fetch the login flow
  use_future(move || {
    let flow_param = flow.clone();
    let nav = nav_for_future;

    async move {
      let config = Configuration::create();

      let result = match flow_param {
        Some(id) => match get_login_flow(&config, &id, None).await {
          Ok(res) => Ok(res),
          // Catch 410 on initial mount
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) if res.status == 410 => {
            expiration_error.set(Some(extract_expired_message(&res.content)));
            nav.replace(Route::LoginFlow { flow: None });
            match create_browser_login_flow(&config, None, None, None, None, None, None, None, None)
              .await
            {
              Ok(fresh) => Ok(fresh),
              Err(ory_kratos_client_wasm::apis::Error::ResponseError(err_res)) => {
                Err(err_res.view_response_content())
              }
              Err(e) => Err(rsx! {
                div { class: "alert alert-error", "Network Error: {e:#?}" }
              }),
            }
          }
          Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
            Err(res.view_response_content())
          }
          Err(e) => Err(rsx! {
            div { class: "alert alert-error", "Network Error: {e:#?}" }
          }),
        },
        None => {
          match create_browser_login_flow(&config, None, None, None, None, None, None, None, None)
            .await
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
      };

      flow_state.set(Some(result));
    }
  });

  // 2. Handle the native async form submission via web-sys
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
                  match serde_wasm_bindgen::from_value::<ory_kratos_client_wasm::models::LoginFlow>(
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
            } else if status == 410 || status == 422 {
              // Extract error before resetting the login pipeline
              let mut msg = "The form expired. A fresh session has been initialized.".to_string();
              if let Ok(text_val) = JsFuture::from(resp.text().unwrap()).await
                && let Some(err_text) = text_val.as_string()
              {
                msg = extract_expired_message(&err_text);
              }
              expiration_error.set(Some(msg));

              nav.replace(Route::LoginFlow { flow: None });
              let config = Configuration::create();
              match create_browser_login_flow(
                &config, None, None, None, None, None, None, None, None,
              )
              .await
              {
                Ok(fresh_flow) => flow_state.set(Some(Ok(fresh_flow))),
                Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
                  flow_state.set(Some(Err(res.view_response_content())));
                }
                Err(e) => flow_state.set(Some(Err(rsx! {
                  div { class: "alert alert-error",
                    "Failed to cycle expired flow: {e:?}"
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
        .unwrap_or_else(|| ("method".to_string(), "password".to_string()));

      rsx! {
        h1 { class: "text-center text-2xl mt-10", "Sign In" }
        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            // Render captured expiration warnings
            if let Some(msg) = expiration_error() {
              Alert {
                variant: AlertVariant::Error,
                persistent: false,
                "{msg}"
              }
            }

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

                  if !parsed_data.contains_key(&submit_strategy.0) {
                      parsed_data.insert(submit_strategy.0.clone(), submit_strategy.1.clone());
                  }

                  submit_form.send((action_url.clone(), parsed_data));
              },
            }
            p { class: "text-sm leading-6 mt-4",
              "Don't have an account? "
              Link {
                to: Route::RegisterFlow { flow: None },
                class: "link-primary link-hover",
                "Register →"
              }
            }
          }
        }
      }
    }
    Some(Err(err_elem)) => {
      rsx! {
        div { class: "mx-auto max-w-lg mt-10", {err_elem.clone()} }
      }
    }
    None => {
      rsx! {
        div { class: "flex justify-center mt-10",
          p { class: "animate-pulse", "Loading login flow..." }
        }
      }
    }
  }
}
