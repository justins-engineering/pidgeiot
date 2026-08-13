use crate::Route;
use dioxus::prelude::*;

#[component]
pub fn Home() -> Element {
  rsx! {
    section { class: "pt-20 pb-14 text-center",
      div { class: "max-w-5xl mx-auto",
        h1 { class: "text-5xl md:text-7xl font-extrabold tracking-tight max-w-4xl mx-auto text-pretty",
          "Carrier pigeons for your sensors."
          br {}
          span { class: "text-primary", "Considerably faster." }
        }
        p { class: "mt-7 text-xl md:text-2xl leading-relaxed max-w-2xl mx-auto text-base-content/80 text-pretty",
          "An open-source platform that provisions your devices, keeps their config and firmware current, and brings their readings home."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-9",
          Link { class: "btn btn-primary btn-lg font-bold", to: Route::DemoPage {},
            "Try the live demo"
          }
          a {
            class: "btn btn-outline btn-lg font-bold",
            href: "https://github.com/justins-engineering",
            target: "_blank",
            rel: "noopener noreferrer",
            "Read the source"
          }
        }
        p { class: "mt-5 text-sm text-base-content/60 font-mono",
          "no signup · no hardware · AGPL-3.0"
        }
      }
    }
  }
}
