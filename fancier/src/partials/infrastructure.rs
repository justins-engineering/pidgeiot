use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdDatabase, LdDatabaseZap, LdSatelliteDish, LdServer};

#[component]
pub fn Infrastructure() -> Element {
  rsx! {
    section { id: "infrastructure", class: "front-page",
      div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-8",
        div { class: "relative col-span-full",
          div { class: "bg-base-300/30 border border-base-300 rounded-3xl overflow-hidden shadow-2xl scroll-reveal",
            div { class: "relative h-64 md:h-80 lg:h-96",
              img {
                alt: "Server Infrastructure",
                class: "w-full h-full object-cover",
                src: "https://images.unsplash.com/photo-1762163516269-3c143e04175c?crop=entropy&cs=tinysrgb&fit=max&fm=jpg&ixid=M3w4MDcxMzN8MHwxfHNlYXJjaHwxfHxzZXJ2ZXIlMjBkYXRhJTIwY2VudGVyJTIwdGVjaG5vbG9neSUyMGluZnJhc3RydWN0dXJlfGVufDB8MHx8fDE3NzE4OTEzMzl8MA&ixlib=rb-4.1.0&q=80&w=1080",
              }
              div { class: "absolute inset-0 bg-linear-to-t from-base-300 via-base-200/50 to-transparent" }
              div { class: "absolute bottom-0 left-0 right-0 p-8",
                div { class: "flex flex-col md:flex-row items-center justify-between",
                  div {
                    p { class: "text-2xl md:text-3xl font-bold mb-2",
                      "Edge-Native, Not Data-Center-Bound"
                    }
                    p {
                      "Every request runs on Cloudflare's edge network — no servers to provision or patch."
                    }
                  }
                }
              }
            }
          }
          div { class: "absolute -top-6 -left-6 w-24 h-24 rounded-2xl bg-primary/30 border border-primary/50 flex items-center justify-center animate-float shadow-lg",
            Icon {
              icon: LdSatelliteDish,
              class: "size-8 stroke-primary",
              title: "Satellite icon",
            }
          }
          div {
            class: "absolute -bottom-6 -right-6 w-24 h-24 rounded-2xl bg-secondary/30 border border-secondary/50 flex items-center justify-center animate-float shadow-lg",
            style: "animation-delay: 1s;",
            Icon {
              icon: LdServer,
              class: "size-8 stroke-secondary",
              title: "Server icon",
            }
          }
        }
        div {
          class: "card card-xl card-border space-y-8 justify-around bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
          style: "animation-delay: 0.2s;",
          div { class: "card-title space-x-4",
            div { class: "p-2 rounded-2xl bg-linear-to-br from-teal-900 to-purple-900 flex items-center justify-center feature-icon shadow-lg",
              Icon {
                icon: LdDatabase,
                class: "size-10 stroke-teal-300",
                title: "Database icon",
              }
            }
            h3 { class: "text-2xl font-bold", "One Durable Object Per Device" }
          }
          p { class: "leading-relaxed",
            "Each pigeon owns a small, SQLite-backed Durable Object — the single source of truth for its shadow, ACL, and device credentials — mirrored best-effort into Postgres via Hyperdrive for cross-device queries."
          }
        }
        div {
          class: "card card-xl card-border space-y-8 justify-around bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
          style: "animation-delay: 0.2s;",
          div { class: "card-title space-x-4",
            div { class: "p-2 rounded-2xl bg-accent flex items-center justify-center feature-icon shadow-lg",
              Icon {
                icon: LdDatabaseZap,
                class: "size-10 stroke-accent-content",
                title: "Database icon",
              }
            }
            div { class: "flex items-center gap-2 flex-wrap",
              h3 { class: "text-2xl font-bold", "No Telemetry Lock-In" }
              span { class: "badge badge-xs badge-primary", "Rolling out" }
            }
          }
          p { class: "leading-relaxed",
            "Point a device at your own GreptimeDB-compatible endpoint and its telemetry goes straight there instead of our default store — your data, your database."
          }
        }
      }
    }
  }
}
