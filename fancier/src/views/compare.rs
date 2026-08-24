use crate::Route;
use crate::components::ComparisonTables;
use crate::helpers::pricing_data::View;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdPlay;

/// The full comparison, split off the pricing page so that page can stay a
/// price list. Everything a buyer would want in order to argue with us
/// lives here instead: every vendor we could price, three fleet sizes, a
/// second cadence, the vendors we could not price at all, and the raw
/// infrastructure rates that beat ours.
#[component]
pub fn ComparePage() -> Element {
  rsx! {
    section { id: "compare-hero", class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Comparison" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "Every price we could find, including the ones that beat ours."
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-3xl text-base-content/80 text-pretty",
          "Vendors price in units that refuse to compare: per device, per message, per datapoint, per event, in blocks. So every figure here is the same device, normalized to what one costs for a month, with each vendor on the cheapest tier that genuinely fits."
        }
        p { class: "mt-4 text-base-content/70",
          "Where a vendor publishes nothing we could use, this page says so rather than estimating."
        }
      }
    }

    section { id: "compare-tables", class: "px-4 md:px-10 py-14",
      div { class: "max-w-5xl mx-auto",
        ComparisonTables { view: View::Full }

        p { class: "mt-12 text-sm",
          Link { class: "link link-hover font-medium", to: Route::PricingPage {},
            "← Back to our own pricing"
          }
        }
      }
    }

    section { id: "compare-cta", class: "px-4 md:px-10 pb-24",
      div { class: "max-w-4xl mx-auto rounded-3xl border border-neutral-content bg-linear-to-br/srgb from-primary/40 via-secondary/40 to-accent/40 p-10 text-center shadow-2xl",
        h2 { class: "text-2xl md:text-3xl font-bold mb-3", "Check the arithmetic yourself." }
        p { class: "text-lg mb-8 leading-relaxed",
          "Every figure links to the page it came from and says when we last read it. If one has moved, tell us and we'll fix it."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-4",
          Link {
            class: "btn btn-lg btn-glow font-bold",
            to: Route::RegisterFlow { flow: None },
            Icon { icon: LdPlay, class: "mr-2", title: "Start free" }
            "Start free"
          }
          a { class: "btn btn-lg btn-outline font-bold", href: "mailto:code@jes.contact",
            "Tell us we're wrong"
          }
        }
      }
    }
  }
}
