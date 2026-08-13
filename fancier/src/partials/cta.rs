use crate::Route;
use dioxus::prelude::*;

#[component]
pub fn Cta() -> Element {
  rsx! {
    section { class: "my-16",
      div {
        class: "max-w-6xl mx-auto rounded-3xl bg-primary px-6 md:px-12 py-16 text-center",
        style: "color:var(--color-primary-content)",
        h2 { class: "text-4xl md:text-5xl font-extrabold tracking-tight", "Send up your first bird" }
        // The design said "the demo flock is already flying"; the public
        // demo is a single allowlisted device.
        p { class: "text-lg md:text-xl mt-4",
          "A real device is already reporting. Ten minutes, no hardware, no card."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-8",
          Link {
            class: "btn btn-lg font-bold border-0",
            style: "background:var(--color-primary-content);color:var(--color-primary)",
            to: Route::DemoPage {},
            "Try the live demo"
          }
          Link {
            class: "btn btn-lg btn-outline font-bold",
            style: "background:transparent;border-color:var(--color-primary-content);color:var(--color-primary-content)",
            to: Route::DocumentationPage {},
            "Read the docs"
          }
        }
      }
    }
  }
}
