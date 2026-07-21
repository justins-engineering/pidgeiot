use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdDatabase, LdDatabaseZap, LdHardDriveDownload, LdSatelliteDish, LdServer,
};

const INFRASTRUCTURE_SVG: &str = include_str!("../../assets/images/infrastructure-edge.svg");

#[component]
pub fn Infrastructure() -> Element {
  rsx! {
    section { id: "infrastructure", class: "front-page",
      div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-8",
        div { class: "relative col-span-full",
          div { class: "bg-base-300/30 border border-base-300 rounded-3xl overflow-hidden shadow-2xl",
            div { class: "relative h-64 md:h-80 lg:h-96",
              div {
                class: "absolute inset-0",
                dangerous_inner_html: "{INFRASTRUCTURE_SVG}",
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
            h3 { class: "text-2xl font-bold", "No Telemetry Lock-In" }
          }
          p { class: "leading-relaxed",
            "Point a device at your own GreptimeDB-compatible endpoint and its telemetry goes straight there instead of our default self-hosted GreptimeDB store — your data, your database."
          }
        }
        div {
          class: "card card-xl card-border space-y-8 justify-around bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
          style: "animation-delay: 0.2s;",
          div { class: "card-title space-x-4",
            div { class: "p-2 rounded-2xl bg-primary flex items-center justify-center feature-icon shadow-lg",
              Icon {
                icon: LdHardDriveDownload,
                class: "size-10 stroke-primary-content",
                title: "Download icon",
              }
            }
            h3 { class: "text-2xl font-bold", "Content-Addressed Firmware" }
          }
          p { class: "leading-relaxed",
            "Firmware images live in R2, addressed by their own SHA-256 and catalogued per flock. Rolling out a version reuses the same shadow model as config — devices resume large downloads with Range requests instead of starting over."
          }
        }
      }
    }
  }
}
