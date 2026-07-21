use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdKeyRound, LdRadio, LdShieldHalf};

#[component]
pub fn Connectivity() -> Element {
  rsx! {
    section { id: "connectivity", class: "front-page",
      div { class: "bg-linear-to-bl/srgb from-primary/40 via-secondary/40 to-accent/40 border border-primary rounded-3xl p-8 md:p-12 shadow-2xl",
        div { class: "flex flex-col lg:flex-row items-center gap-12",
          div { class: "lg:w-2/3",
            h2 { class: "text-3xl md:text-4xl font-bold mb-6",
              "Built for Constrained, Cellular Hardware"
            }
            p { class: "text-xl mb-8 leading-relaxed",
              "Every byte costs money and battery on a cellular device. PidgeIoT is designed around small, auditable wire formats instead of heavyweight standards — maximum security with minimal data transfer."
            }
            div { class: "grid grid-cols-1 md:grid-cols-3 gap-5",
              div { class: "flex flex-row space-x-2 items-start p-4 rounded-xl bg-base-300/50 border border-primary/40 hover:border-primary transition-all duration-300",
                Icon {
                  icon: LdKeyRound,
                  class: "mt-1 h-8 w-1/2 stroke-primary",
                  title: "Key icon",
                }
                div {
                  h3 { class: "font-bold text-lg mb-2", "69-Byte Tokens" }
                  p { class: "text-sm",
                    "A compact binary bearer token — version, expiry, signature — verified with one Ed25519 check at the edge. No JWT overhead."
                  }
                }
              }
              div { class: "flex flex-row space-x-2 items-start p-4 rounded-xl bg-base-300/50 border border-secondary/40 hover:border-secondary transition-all duration-300",
                Icon {
                  icon: LdShieldHalf,
                  class: "mt-1 h-8 w-1/2 text-secondary",
                  title: "Shield icon",
                }
                div {
                  h3 { class: "font-bold text-lg mb-2", "CoAP-over-TLS/TCP" }
                  p { class: "text-sm",
                    "RFC 8323 CoAP for devices too constrained for a full HTTPS stack — still fully encrypted, no bare UDP."
                  }
                }
              }
              div { class: "flex flex-row space-x-2 items-start p-4 rounded-xl bg-base-300/50 border border-primary/40 hover:border-primary transition-all duration-300",
                Icon {
                  icon: LdRadio,
                  class: "mt-1 h-8 w-1/2 stroke-primary",
                  title: "Radio icon",
                }
                div {
                  h3 { class: "font-bold text-lg mb-2", "Dictionary Logging" }
                  p { class: "text-sm",
                    "Structured device logs from our Zephyr library, shipped as dictionary-compressed codes instead of raw strings."
                  }
                }
              }
            }
          }
          div { class: "lg:w-1/3 flex justify-center",
            div { class: "relative",
              div { class: "w-48 h-48 rounded-full bg-linear-to-br from-primary/20 to-secondary/20 flex items-center justify-center animate-spin-slow border border-primary/30",
                div { class: "w-36 h-36 rounded-full bg-linear-to-br from-primary/30 to-secondary/30 flex items-center justify-center",
                  Icon {
                    icon: LdRadio,
                    class: "size-20 stroke-primary",
                    title: "Radio icon",
                  }
                }
              }
              div { class: "absolute -bottom-4 -right-4 w-16 h-16 rounded-full bg-secondary flex items-center justify-center shadow-lg animate-bounce-slow",
                Icon {
                  icon: LdShieldHalf,
                  class: "size-7 stroke-secondary-content",
                  title: "Shield icon",
                }
              }
            }
          }
        }
      }
    }
  }
}
