use dioxus::prelude::*;

const ARCHITECTURE_SVG: &str = include_str!("../../assets/images/architecture.svg");

#[component]
pub fn Architecture() -> Element {
  rsx! {
    // Main container: full height, centered content, using your base background
    div { class: "w-full flex flex-col items-center justify-center",

      h1 { class: "text-2xl md:text-4xl font-bold text-base-content mb-3 text-center",
        "PidgeIoT System Architecture"
      }

      p { class: "text-base text-base-content/70 max-w-2xl text-center mb-8",
        "A complete topology mapping the data flow from constrained edge devices, through Cloudflare's global compute network, down to self-hosted Proxmox storage."
      }

      // 3. Diagram
      div {
        // Constrains the max width so it fits nicely on desktop monitors without blowing up
        class: "w-full max-h-screen flex grow justify-center",
        dangerous_inner_html: "{ARCHITECTURE_SVG}",
      }
    }
  }
}
