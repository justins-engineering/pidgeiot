use crate::Route;
use dioxus::prelude::*;

#[component]
pub fn OpenSource() -> Element {
  rsx! {
    section { class: "py-16",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 md:grid-cols-3 gap-8",

        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Open, all of it" }
          p { class: "leading-relaxed text-base-content/80",
            "AGPL-3.0 across the edge router, the dashboard and the device library. No open core with the useful parts behind a sales call."
          }
          a {
            class: "link link-primary font-semibold text-sm",
            href: "https://github.com/justins-engineering",
            target: "_blank",
            rel: "noopener noreferrer",
            "Browse the repos →"
          }
        }

        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Secure by shape" }
          p { class: "leading-relaxed text-base-content/80",
            "Per-device Ed25519 keys, encrypted transports only, and a token so small it costs nothing to send. One compromised device stays one compromised device."
          }
          Link { class: "link link-primary font-semibold text-sm", to: Route::HowItWorksPage {},
            "Security model →"
          }
        }

        // The design said telemetry sent to your own endpoint is never
        // stored. The latest value per key is always kept — it's what the
        // dashboard renders and what alerts evaluate against — so the
        // claim is scoped to the history, which is the part that's true.
        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Private by default" }
          p { class: "leading-relaxed text-base-content/80",
            "Send telemetry to your own endpoint and the history accumulates there, not here — we keep only the latest value per key. Dashboard identity is self-hosted, so your credentials don't visit a third party."
          }
          Link { class: "link link-primary font-semibold text-sm", to: Route::HowItWorksPage {},
            "How data flows →"
          }
        }
      }
    }
  }
}
