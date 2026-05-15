use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdFileCode, LdGift, LdPlay};

#[component]
pub fn Cta() -> Element {
  rsx! {
    section { id: "cta", class: "front-page",
      div { class: "bg-linear-to-br/srgb from-primary/40 via-secondary/40 to-accent/40 border border-neutral-content rounded-3xl p-8 md:p-12 text-center shadow-2xl scroll-reveal",
        h2 { class: "text-3xl md:text-4xl lg:text-5xl font-bold mb-6",
          "Ready to Build Secure IoT Solutions?"
        }
        p { class: "text-xl mb-10 leading-relaxed",
          "Join us in creating the next generation of IoT management."
        }
        div { class: "flex flex-col sm:flex-row justify-center space-y-4 sm:space-y-0 sm:space-x-6 mb-8",
          Link {
            class: "btn btn-xl btn-glow sm:w-1/2 grow font-bold",
            to: Route::SignUp {},
            Icon {
              icon: LdPlay,
              class: "mr-2 animate-pulse-slow",
              title: "Start Now",
            }
            "Start Now"
          }
          a {
            class: "btn btn-xl btn-special sm:w-1/2 grow font-bold",
            href: "https://github.com/justins-engineering",
            Icon {
              icon: LdFileCode,
              class: "mr-2",
              title: "Read the Documentation",
            }
            "Read the Docs"
          }
        }
        div { class: "inline-flex items-center px-4 py-2 rounded-full bg-base-300/30 border border-primary/30",
          Icon {
            icon: LdGift,
            class: "size-10 sm:size-5 mr-2 stroke-primary",
            title: "Gift",
          }
          p { class: "text-sm", "No credit card required." }
        }
      }
    }
  }
}
