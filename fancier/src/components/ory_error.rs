use crate::Route;
use dioxus::prelude::*;
use ory_kratos_client_wasm::models::error_generic::ErrorGeneric;

// 1. Converted to proper Dioxus components to enable prop diffing and memoization
#[component]
pub fn ErrorContentRsx(err: ErrorGeneric) -> Element {
  // The SDK guarantees message is a String, so we use it directly
  let message = err.error.message;

  // Reason is typically an Option<String> in the Kratos schema
  let reason = err.error.reason.unwrap_or_default();

  rsx! {
    div { class: "text-center max-h-screen max-w-none",
      h1 { class: "text-2xl my-8 capitalize", "{message}" }
      if !reason.is_empty() {
        p { class: "font-light m-8", "{reason}" }
      }
      Link { to: Route::Index {}, class: "btn btn-primary my-8", "Go Home" }
    }
  }
}

#[component]
pub fn ErrorContentJs(err: serde_json::Value) -> Element {
  // 3. Fallback to full JSON stringification if the value is an object/array
  let error_text = err
    .as_str()
    .map(|s| s.to_string())
    .unwrap_or_else(|| err.to_string());

  rsx! {
    div { class: "text-center max-h-screen max-w-none flex flex-col items-center",
      // Added whitespace-pre-wrap so raw JSON formatting is actually readable
      pre { class: "font-light m-8 text-left text-sm max-w-2xl whitespace-pre-wrap overflow-x-auto",
        "{error_text}"
      }
      Link { to: Route::Index {}, class: "btn btn-primary my-8", "Go Home" }
    }
  }
}
