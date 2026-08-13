use crate::Route;
use dioxus::prelude::*;

struct Card {
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
}

const CARDS: [Card; 5] = [
  Card {
    eyebrow: "Fleet",
    title: "Vehicle & asset tracking",
    body: "Tracks drawn from plain GPS telemetry. Alerts when an asset moves — or stops reporting.",
  },
  Card {
    eyebrow: "Farm",
    title: "Irrigation & soil",
    body: "Moisture per block, valves as config, a season of battery between visits.",
  },
  Card {
    eyebrow: "Factory",
    title: "Machine monitoring",
    body: "Vibration trends, rate-of-change alarms, remote logs when something's off.",
  },
  Card {
    eyebrow: "Utility",
    title: "Water metering",
    body: "Small payloads, sent reliably, with history in your own database if you prefer.",
  },
  // The design promised "same shape at 5 units or 50,000". The
  // architecture holds, but the dashboard has no paginated device list
  // yet, so the claim is made about the edge model rather than a fleet
  // size we can't render.
  Card {
    eyebrow: "City",
    title: "Smart parking",
    body: "Bay occupancy served from the edge nearest each sensor — one object per device, however many there are.",
  },
];

#[component]
pub fn UseCaseStrip() -> Element {
  rsx! {
    section { class: "py-16",
      div { class: "max-w-6xl mx-auto",
        h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
          "Where it earns its keep"
        }
        p { class: "text-lg text-base-content/70 mt-2 mb-9",
          "Examples, not case studies — we're in beta and we're not going to pretend otherwise."
        }
        div { class: "grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4",
          for c in CARDS.iter() {
            div {
              key: "{c.title}",
              class: "rounded-2xl bg-base-200 border border-base-300 p-6 flex flex-col gap-3 min-w-0",
              span { class: "text-xs font-mono uppercase tracking-widest text-primary", "{c.eyebrow}" }
              h3 { class: "text-lg font-bold leading-snug", "{c.title}" }
              p { class: "text-sm leading-relaxed text-base-content/75", "{c.body}" }
            }
          }
        }
        div { class: "mt-8",
          Link { class: "link link-primary font-semibold", to: Route::UseCasesPage {},
            "See what each one uses →"
          }
        }
      }
    }
  }
}
