use crate::helpers::DisplayError;
use crate::{Configuration, Create};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::to_session;

#[component]
pub fn SessionInfo() -> Element {
  let get_session = use_resource(move || async move {
    let config = Configuration::create();
    match to_session(&config, None, None, None).await {
      Ok(res) => Ok(res),
      Err(ory_kratos_client_wasm::apis::Error::ResponseError(res)) => {
        Err(res.view_response_content())
      }
      Err(e) => Err(rsx! {
        div { class: "alert alert-error", "Network Error: {e:#?}" }
      }),
    }
  });

  match &*get_session.read() {
    Some(Ok(res)) => {
      rsx! {
        h1 { class: "text-center text-2xl mt-10", "Session Info" }
        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            label { class: "text-lg", "Basic Info" }
            table { class: "table",
              tbody {
                tr {
                  th { "ID" }
                  td { {res.id.clone()} }
                }
                if let Some(active) = res.active {
                  tr {
                    th { "Active" }
                    td { {active.to_string()} }
                  }
                }
                if let Some(authenticated_at) = &res.authenticated_at {
                  tr {
                    th { "Authenticated" }
                    td { {authenticated_at.clone()} }
                  }
                }
                if let Some(authenticator_assurance_level) = &res.authenticator_assurance_level {
                  tr {
                    th { "Authenticator Assurance Level" }
                    td { {authenticator_assurance_level.to_string()} }
                  }
                }
                if let Some(expires_at) = &res.expires_at {
                  tr {
                    th { "Expires" }
                    td { {expires_at.clone()} }
                  }
                }
                if let Some(issued_at) = &res.issued_at {
                  tr {
                    th { "Issued" }
                    td { {issued_at.clone()} }
                  }
                }
                if let Some(tokenized) = &res.tokenized {
                  tr {
                    th { "Tokenized" }
                    td { {tokenized.clone()} }
                  }
                }
              }
            }
          }
        }
        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            label { class: "text-lg", "Authentication Methods" }
            table { class: "table",
              thead {
                tr {
                  th { "AAL" }
                  th { "Completed At" }
                  th { "Method" }
                  th { "Organization" }
                  th { "Provider" }
                }
              }
              if let Some(authentication_methods) = &res.authentication_methods {
                tbody {
                  for method in authentication_methods {
                    tr {
                      td {
                        match method.aal {
                            Some(aal) => aal.to_string(),
                            None => "".to_string(),
                        }
                      }
                      td {
                        match &method.completed_at {
                            Some(completed_at) => completed_at,
                            None => "",
                        }
                      }
                      td {
                        match method.method {
                            Some(method) => format!("{method:?}"),
                            None => "".to_string(),
                        }
                      }
                      td {
                        match &method.organization {
                            Some(organization) => organization,
                            None => "",
                        }
                      }
                      td {
                        match &method.provider {
                            Some(provider) => provider,
                            None => "",
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            label { class: "text-lg", "Devices" }
            table { class: "table",
              thead {
                tr {
                  th { "ID" }
                  th { "IP Address" }
                  th { "Location" }
                  th { "User Agent" }
                }
              }
              if let Some(devices) = &res.devices {
                tbody {
                  for device in devices {
                    tr {
                      td { {device.id.clone()} }
                      td {
                        match &device.ip_address {
                            Some(ip_address) => ip_address,
                            None => "",
                        }
                      }
                      td {
                        match &device.location {
                            Some(location) => location,
                            None => "",
                        }
                      }
                      td {
                        match &device.user_agent {
                            Some(user_agent) => user_agent,
                            None => "",
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }

        div { class: "mx-auto w-full max-w-lg",
          div { class: "mt-10",
            label { class: "text-lg", "Identity" }
            if let Some(identity) = &res.identity {
              pre { class: "whitespace-pre-wrap overflow-x-auto text-xs bg-base-200 p-4 rounded",
                {format!("{identity:#?}")}
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
        p { class: "animate-pulse", "Loading session info..." }
      }
    },
  }
}
