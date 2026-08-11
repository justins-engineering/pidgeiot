use crate::Route;
use crate::components::{Maturity, MaturityBadge};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdKeyRound, LdMail, LdScrollText, LdServer};

/// The investors/incubators section — deliberately placed after every
/// user-facing section and deliberately quieter than them (no gradient
/// boxes, no animation): the page sells to individual builders first, and
/// this section only states architecture and position that the rest of the
/// page has already demonstrated. Hard rule: no invented traction,
/// customers, or numbers here, ever. The roadmap item carries a `Planned`
/// badge for the same reason.
#[component]
pub fn Why() -> Element {
  rsx! {
    section { id: "platform", class: "front-page",
      div { class: "max-w-4xl mx-auto",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2 text-center",
          "The long view"
        }
        h2 { class: "text-3xl md:text-4xl font-bold mb-6 text-center", "Why This Platform" }
        p { class: "text-lg leading-relaxed mb-12 text-center text-pretty",
          "Open-source IoT today makes builders choose: assemble a pile of primitives yourself, or pay enterprise prices for the pre-assembled version. PidgeIoT's bet is that one coherent, AGPL-licensed product — identity, config, firmware, telemetry, and alerts designed together in a single codebase — wins the individual developers that the incumbents price out or wear down. Those developers become the small fleets, and the small fleets become the large ones."
        }
        div { class: "space-y-8",
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdServer,
                class: "size-7 stroke-primary",
                title: "Server icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Serverless economics, edge-native by default" }
              p { class: "leading-relaxed text-base-content/80",
                "The backend runs on Cloudflare Workers and Durable Objects — each device owns its own SQLite-backed object at the edge. No idle servers to pay for, no capacity planning: a fleet of five costs almost nothing to serve, and the same architecture serves a fleet of thousands without a re-platform."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdKeyRound,
                class: "size-7 stroke-primary",
                title: "Key icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Cryptographic identity per device" }
              p { class: "leading-relaxed text-base-content/80",
                "Every device authenticates with its own Ed25519 keypair and a 69-byte binary token — no shared secrets, no JWT overhead, and refreshing a token is revocation, because it overwrites the only key the old one could verify against. Dashboard identity is self-hosted Ory Kratos: user credentials never leave infrastructure we control."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdScrollText,
                class: "size-7 stroke-primary",
                title: "Scroll icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Rust and WebAssembly, end to end" }
              p { class: "leading-relaxed text-base-content/80",
                "The edge router, this dashboard, and the wire types between them are one Rust workspace — the backend compiles to a Worker, the frontend to WebAssembly, and shared structs mean the two cannot drift apart. The protocol itself is the product surface: everything the dashboard does rides the same documented API a device or a script can use."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-b border-base-content/10 py-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdServer,
                class: "size-7 stroke-secondary",
                title: "Server icon",
              }
            }
            div {
              div { class: "flex items-center gap-3 flex-wrap mb-2",
                h3 { class: "text-xl font-bold", "Next: a user-authored rule engine" }
                MaturityBadge { maturity: Maturity::Planned }
              }
              p { class: "leading-relaxed text-base-content/80",
                "Designed, not yet built: user-written logic running against incoming telemetry at the edge, on Cloudflare Workers for Platforms — the step from device management to a programmable platform."
              }
            }
          }
        }
        div { class: "mt-12 text-center",
          p { class: "leading-relaxed text-base-content/70 max-w-2xl mx-auto mb-6",
            "PidgeIoT is in beta and pre-revenue, and this page says so. There are no customer logos here because we haven't earned them yet — the public repos, the commit history, and the running product are the evidence. If you're evaluating us, read the code."
          }
          div { class: "flex flex-col sm:flex-row justify-center gap-4",
            Link {
              class: "btn btn-outline rounded-full font-bold",
              to: Route::Architecture {},
              "Read the Architecture"
            }
            a {
              class: "btn btn-outline rounded-full font-bold",
              href: "mailto:code@jes.contact",
              Icon { icon: LdMail, class: "mr-2", title: "Email" }
              "Talk to Us"
            }
          }
        }
      }
    }
  }
}
