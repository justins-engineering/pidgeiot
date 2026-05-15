use crate::{Configuration, Create, Route};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::get_flow_error;

#[component]
pub fn PageNotFound(route: Vec<String>) -> Element {
  rsx! {
    div { class: "text-center max-h-screen max-w-none",
      h1 { class: "text-9xl my-12", "404" }
      h2 { class: "text-5xl my-8", "Page not found" }
      h3 { class: "text-xl my-8",
        "The page {route:?} might have been removed or is temporarily unavailable."
      }
      Link { to: Route::Index {}, class: "btn btn-lg btn-primary btn-wide", "Go Home" }
    }
  }
}

#[component]
pub fn Unauthorized() -> Element {
  rsx! {
    div { class: "text-center max-h-screen max-w-none",
      h1 { class: "text-9xl my-12", "401" }
      h2 { class: "text-5xl my-8", "Unauthorized" }
      h3 { class: "text-xl my-8",
        "Your request was not successful because it lacks valid authentication credentials."
      }
      Link {
        to: Route::SignIn {},
        class: "btn btn-lg btn-primary btn-wide",
        "Sign In"
      }
    }
  }
}

#[component]
pub fn ServerError(id: String) -> Element {
  let err_id = id.clone();
  let future = use_resource(move || {
    let err_id = err_id.to_owned();
    async move { get_flow_error(&Configuration::create(), &err_id).await }
  });

  rsx! {
    div { class: "text-center max-h-screen max-w-none",
      h1 { class: "text-2xl my-8", "Oops! We've encountered an error." }
      h2 { class: "font-light my-8", "Error ID: {id}" }

      match &*future.read_unchecked() {
          Some(Ok(res)) => {
              match &res.error {
                  Some(Value) => {
                      if Value.is_object() {
                          rsx! {
                            for (key , value) in Value.as_object().unwrap() {
                              p { class: "font-light m-8", "{key}: {value}" }
                            }
                          }
                      } else {
                          rsx! {
                            p { class: "font-light m-8", "Error: {Value:?}" }
                          }
                      }
                  }
                  None => rsx! {},
              }
          }
          Some(Err(err)) => rsx! {
            p { class: "font-light m-8", "Failed to get error: {err}" }
          },
          None => rsx! {
            p { class: "font-light my-8", "Fetching error..." }
          },
      }
      Link { to: Route::Index {}, class: "btn btn-primary my-8", "Go Home" }
    }
  }
}
