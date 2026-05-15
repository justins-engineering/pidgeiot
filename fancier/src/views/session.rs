use std::format;

use crate::components::DisplayError;
use crate::{Configuration, Create};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::to_session;

#[component]
pub fn SessionInfo() -> Element {
  let create_flow: Resource<Result<_, ory_kratos_client_wasm::apis::Error<_>>> = use_resource(
    move || async move { to_session(&Configuration::create(), None, None, None).await },
  );

  return match &*create_flow.read() {
    Some(new_flow) => match new_flow {
      Ok(res) => {
        rsx! {
          h1 { class: "text-center text-2xl", "Session Info" }
          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              label { class: "text-lg",
                "Basic Info"
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
          }
          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              label { class: "text-lg",
                "Authentication Methods"
                table { class: "table",
                  thead {
                    th { "AAL" }
                    th { "Completed At" }
                    th { "Method" }
                    th { "Organization" }
                    th { "Provider" }
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
          }
          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              label { class: "text-lg",
                "Devices"
                table { class: "table",
                  thead {
                    th { "ID" }
                    th { "IP Address" }
                    th { "Location" }
                    th { "User Agent" }
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
          }

          div { class: "mx-auto w-full max-w-lg",
            div { class: "mt-10",
              label { class: "text-lg",
                "Identity"
                if let Some(identity) = &res.identity {
                  code { {format!("{identity:#?}")} }
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
          p { "Failed to get Session! Error:" }
          p { "{err:#?}" }
        }
      }
    },
    None => rsx! {},
  };
}

// div { class: "mx-auto w-full max-w-lg",
//   div { class: "mt-10",
//     label { class: "text-lg",
//       "Identity"
//       table { class: "table",
//         thead {
//           th { "ID" }
//           th { "created_at" }
//           th { "credentials" }
//           th { "metadata_admin" }
//           th { "metadata_public" }
//           th { "organization_id" }
//           th { "recovery_addresses" }
//           th { "schema_id" }
//           th { "schema_url" }
//           th { "state" }
//           th { "state_changed_at" }
//           th { "traits" }
//           th { "updated_at" }
//           th { "verifiable_addresses" }
//         }
//         if let Some(*identity) = res.identity {
//           tbody {
//             for id in identity {
//               tr {
//                 td {
//                   match id.aal {
//                       Some(aal) => aal.to_string(),
//                       None => "".to_string(),
//                   }
//                 }
//                 td {
//                   match &id.completed_at {
//                       Some(completed_at) => completed_at.clone(),
//                       None => "".to_string(),
//                   }
//                 }
//                 td {
//                   match id.method {
//                       Some(method) => format!("{method:?}"),
//                       None => "".to_string(),
//                   }
//                 }
//                 td {
//                   match &id.organization {
//                       Some(organization) => organization.clone(),
//                       None => "".to_string(),
//                   }
//                 }
//                 td {
//                   match &id.provider {
//                       Some(provider) => provider.clone(),
//                       None => "".to_string(),
//                   }
//                 }
//               }
//             }
//           }
//         }
//       }
//     }
//   }
// }
