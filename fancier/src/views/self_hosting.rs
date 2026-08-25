use crate::Route;
use crate::components::ComparisonTable;
use crate::helpers::pricing_data::{self, View};
use crate::helpers::tco_data;
use dioxus::prelude::*;

/// The answer to "why would I pay you when mosquitto and postgres are
/// free". They are free, and the page says so; what it prices is the part
/// of the bill that arrives as time rather than an invoice.
#[component]
pub fn SelfHostingPage() -> Element {
  let mut tco = use_signal(tco_data::baked);

  // Same arrangement as the comparison tables: never runs during the
  // synchronous SSG pass, so the prerendered page carries the baked
  // figures, and the post-hydration write is also what re-evaluates
  // staleness against the reader's date rather than the build's.
  use_future(move || async move {
    let published = tco_data::fetch_published().await;
    tco.set(published.unwrap_or_else(tco_data::baked));
  });

  let data = tco();
  let today = pricing_data::today();
  let stale_after = data.stale_after_days;

  rsx! {
    section { id: "self-hosting-hero", class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-5xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Self-hosting" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "\"I could run mosquitto, postgres and grafana myself for nothing.\""
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-3xl text-base-content/80 text-pretty",
          "You could, and the software really is free. Here is the rest of the bill, which arrives as your time rather than an invoice."
        }
      }
    }

    section { id: "self-hosting-costs", class: "px-4 md:px-10 py-14",
      div { class: "max-w-5xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight", "{data.heading}" }
        p { class: "mt-3 text-base-content/70 leading-relaxed", "{data.subhead}" }

        ComparisonTable {
          first_heading: "Option",
          columns: data.columns.clone(),
          rows: data.rows.clone(),
          view: View::Full,
          today,
          stale_after,
        }
      }
    }

    section { id: "self-hosting-hours", class: "px-4 md:px-10 py-14 bg-base-200 border-y border-base-300",
      div { class: "max-w-3xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight", "{data.hours.heading}" }
        p { class: "mt-4 text-lg text-base-content/80 leading-relaxed", "{data.hours.body}" }
        p { class: "mt-4 text-base-content/70 leading-relaxed",
          "We are not going to tell you how many hours it takes, because it depends on your stack and your luck. You already know roughly what yours are, and that is the number that decides this."
        }
        // The argument against our own page. A comparison that only runs
        // one way reads as an advertisement, and a reader who spots that
        // stops believing the table above it too.
        p { class: "mt-6 rounded-2xl border border-base-300 bg-base-100 p-5 leading-relaxed",
          "{data.concession}"
        }
      }
    }

    section { id: "self-hosting-exit", class: "px-4 md:px-10 py-14",
      div { class: "max-w-3xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight", "{data.exit.heading}" }
        p { class: "mt-4 text-lg text-base-content/80 leading-relaxed", "{data.exit.body}" }
        p { class: "mt-8 text-sm",
          Link { class: "link link-hover font-medium", to: Route::ComparePage {},
            "How we compare against the hosted platforms →"
          }
        }
      }
    }

    section { id: "self-hosting-cta", class: "px-4 md:px-10 pb-24",
      div { class: "max-w-4xl mx-auto rounded-3xl border border-neutral-content bg-linear-to-br/srgb from-primary/40 via-secondary/40 to-accent/40 p-10 text-center shadow-2xl",
        h2 { class: "text-2xl md:text-3xl font-bold mb-3", "Try it before you build it." }
        p { class: "text-lg mb-8 leading-relaxed",
          "Ten devices are free, permanently. If it saves you the weekend, keep it; if it does not, you have lost an afternoon and learned what your own stack would have cost."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-4",
          Link {
            class: "btn btn-lg btn-glow font-bold",
            to: Route::RegisterFlow { flow: None },
            "Start free"
          }
          Link { class: "btn btn-lg btn-outline font-bold", to: Route::PricingPage {},
            "See the pricing"
          }
        }
      }
    }
  }
}
