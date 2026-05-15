use dioxus::prelude::*;
use time::OffsetDateTime;

#[component]
pub fn PigeonView(flock_id: String, pigeon_id: String) -> Element {
  rsx! {
    div { class: "max-w-5xl mx-auto w-full",
      PigeonInfo { flock_id, pigeon_id }
    }
  }
}

#[component]
fn PigeonInfo(flock_id: String, pigeon_id: String) -> Element {
  match use_context::<crate::LocalSession>()
    .pigeons
    .read()
    .get(&pigeon_id)
  {
    Some(pigeon) => {
      rsx! {
        div { class: "flex flex-col gap-6",

          // Header Card
          div { class: "flex flex-col md:flex-row justify-between items-start md:items-center gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
            div {
              h1 { class: "text-3xl font-bold flex items-center gap-3",
                "{pigeon.name}"
                if pigeon.last_connected.is_some() {
                  span { class: "badge badge-success badge-sm", "Connected" }
                } else {
                  span { class: "badge badge-ghost badge-sm", "Offline" }
                }
              }
              p { class: "text-base-content/60 text-sm mt-1 font-mono",
                "ID: {pigeon.id}"
              }
            }
            button {
              class: "btn btn-primary btn-glow",
              onclick: move |_| {
                  document::eval(r#"document.getElementById("send_nidd_modal").showModal();"#);
              },
              "Send NIDD"
            }
          }

          // Properties Grid
          div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4",
            StatCard {
              title: "Serial Number",
              value: pigeon.serial.clone(),
            }
            StatCard {
              title: "Connector",
              value: pigeon.connector.clone(),
            }
            StatCard { title: "Location", value: pigeon.location.clone() }
            StatCard { title: "Tags", value: pigeon.tags.clone() }
          }

          // Timestamps Section
          div { class: "bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm mt-4",
            h3 { class: "text-lg font-semibold mb-4 border-b border-base-content/10 pb-2",
              "System Activity"
            }
            div { class: "flex flex-col",
              TimeRow {
                label: "Last Connected",
                timestamp: pigeon.last_connected,
              }
              TimeRow {
                label: "Last Updated",
                timestamp: pigeon.updated_at,
              }
              TimeRow {
                label: "Created On",
                timestamp: pigeon.created_at,
              }
            }
          }
        }
      }
    }
    None => rsx! {
      div { class: "alert alert-warning shadow-lg",
        span { "Pigeon not found or loading data..." }
      }
    },
  }
}

// UI Sub-components for clean code
#[component]
fn StatCard(title: String, value: Option<String>) -> Element {
  rsx! {
    div { class: "bg-base-100 p-5 rounded-box border border-base-content/10 shadow-sm flex flex-col gap-1",
      span { class: "text-base-content/60 text-sm font-medium", "{title}" }
      span { class: "text-lg font-semibold text-base-content truncate",
        "{value.as_deref().unwrap_or(\"--\")}"
      }
    }
  }
}

#[component]
fn TimeRow(label: String, timestamp: Option<OffsetDateTime>) -> Element {
  let display_time = match timestamp {
    Some(dt) => {
      // The format description is compiled statically for maximum performance
      let format = time::macros::format_description!(
        "[month repr:short] [day padding:none], [year] at [hour]:[minute]:[second] UTC"
      );

      // Directly format the OffsetDateTime
      dt.format(&format)
        .unwrap_or_else(|_| "Invalid Format".to_string())
    }
    None => "--".to_string(),
  };

  rsx! {
    div { class: "flex justify-between items-center py-3 border-b border-base-content/5 last:border-0",
      span { class: "text-base-content/70 font-medium", "{label}" }
      span { class: "text-base-content font-mono text-sm bg-base-200 px-2 py-1 rounded",
        "{display_time}"
      }
    }
  }
}

// use dioxus::prelude::*;

// #[component]
// pub fn PigeonView(flock_id: i64, pigeon_id: i64) -> Element {
//   // let Some(pigeon) = use_context::<crate::LocalSession>().pigeons.read().get(&id);
//   rsx! {
//     PigeonInfo { flock_id, pigeon_id }
//   }
// }

// #[component]
// fn PigeonInfo(flock_id: i64, pigeon_id: i64) -> Element {
//   match use_context::<crate::LocalSession>()
//     .pigeons
//     .read()
//     .get(&pigeon_id)
//   {
//     Some(pigeon) => {
//       rsx! {
//         // div { class: "my-5 flex flex-row justify-around items-center",
//         //   if pigeon.connected {
//         //     p { class: "text-2xl",
//         //       span { class: "status status-lg status-success" }
//         //       " Connected"
//         //     }
//         //   } else {
//         //     p { class: "text-2xl",
//         //       span { class: "status status-lg" }
//         //       " Disconnected"
//         //     }
//         //   }
//         //   button {
//         //     class: "btn btn-outline",
//         //     onclick: move |_| {
//         //         document::eval(r#"document.getElementById("send_nidd_modal").showModal();"#);
//         //     },
//         //     "Send NIDD"
//         //   }
//         // }

//         div { class: "my-5",
//           h2 { class: "text-2xl", "Activity" }
//           div { class: "overflow-x-auto rounded-box border border-base-content/5",
//             table { class: "table",
//               thead {
//                 tr {
//                   th { class: "border-r border-base-content/5", "Name" }
//                   th { class: "border-r border-base-content/5", "Serial" }
//                   th { class: "border-r border-base-content/5", "Tags" }
//                   th { class: "border-r border-base-content/5", "Connector" }
//                   th { class: "border-r border-base-content/5", "Location" }
//                   th { class: "border-r border-base-content/5", "Last Connected" }
//                   th { class: "border-r border-base-content/5", "Connector" }
//                   th { class: "border-r border-base-content/5", "Updated" }
//                   th { class: "border-r border-base-content/5", "Created" }
//                 }
//               }
//               tbody {
//                 tr {
//                   td { class: "border-r border-base-content/5", "{pigeon.name}" }
//                   for some_str in [
//                       &pigeon.serial,
//                       &pigeon.connector,
//                       &pigeon.location,
//                       &pigeon.tags,
//                       &pigeon.connector,
//                       &pigeon.location,
//                   ]
//                   {
//                     if let Some(str) = some_str {
//                       td { class: "border-r border-base-content/5",
//                         "{str}"
//                       }
//                     } else {
//                       td { class: "border-r border-base-content/5",
//                         ""
//                       }
//                     }
//                   }
//                   for some_i64 in [&pigeon.last_connected, &pigeon.updated_at, &pigeon.created_at] {
//                     if let Some(i64) = some_i64 {
//                       td { class: "border-r border-base-content/5",
//                         "{i64}"
//                       }
//                     } else {
//                       td { class: "border-r border-base-content/5",
//                         ""
//                       }
//                     }
//                   }
//                 }
//               }
//             }
//           }
//         }
//       }
//     }
//     None => rsx!(),
//   }
// }
