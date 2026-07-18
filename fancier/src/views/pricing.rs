use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdGift, LdMail, LdPlay};

#[component]
pub fn PricingPage() -> Element {
  rsx! {
    section { class: "py-24 md:py-32 text-center",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        Icon {
          icon: LdGift,
          class: "w-12 h-12 mx-auto mb-8 stroke-primary",
          title: "Gift",
        }
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "Free While We're in Beta"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-2xl mx-auto text-balance mb-4",
          "PidgeIoT is under active development. Every feature on this site is free to use right now — no credit card, no trial countdown."
        }
        p { class: "text-lg text-base-content/60 leading-relaxed max-w-2xl mx-auto text-balance",
          "Paid tiers are planned for after beta, once the platform's core surface (device shadows, telemetry, firmware, logging) has settled. We haven't set that pricing yet, and we're not going to publish numbers before they're real."
        }
      }
    }

    section { class: "pb-24 md:pb-32",
      div { class: "max-w-2xl mx-auto px-4 md:px-8",
        div { class: "bg-linear-to-br/srgb from-primary/40 via-secondary/40 to-accent/40 border border-neutral-content rounded-3xl p-10 text-center shadow-2xl",
          h2 { class: "text-2xl md:text-3xl font-bold mb-4", "Beta Access" }
          p { class: "text-lg mb-8 leading-relaxed",
            "Register a dashboard account and start provisioning pigeons today. If you're planning a larger deployment and want a heads-up before pricing lands, reach out."
          }
          div { class: "flex flex-col sm:flex-row justify-center gap-4",
            Link {
              class: "btn btn-lg btn-glow font-bold",
              to: Route::RegisterFlow { flow: None },
              Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
              "Start Now, Free"
            }
            a {
              class: "btn btn-lg btn-outline font-bold",
              href: "mailto:code@jes.contact",
              Icon { icon: LdMail, class: "mr-2", title: "Email" }
              "Contact Us"
            }
          }
        }
      }
    }
  }
}
