use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdCheck, LdCodeXml, LdCopyleft, LdLockKeyhole, LdX};

#[component]
pub fn Features() -> Element {
  rsx! {
    section { id: "features", class: "front-page",
      div { class: "text-center mb-4",
        h2 { class: "text-3xl md:text-4xl lg:text-5xl font-bold mb-4",
          "Built for the Builders"
        }
        br {}
        p { class: "text-xl max-w-3xl mx-auto",
          "A complete IoT platform built to work for you. No need to start from scratch. We've created simple API meant to easily integrate with any device, and the tech stack you're already using."
        }
      }
      div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-8",
        div {
          class: "card card-border space-y-8 justify-around bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
          style: "animation-delay: 0.2s;",
          div { class: "card-title space-x-4",
            div { class: "p-2 rounded-2xl bg-primary-content feature-icon shadow-lg",
              Icon {
                icon: LdCodeXml,
                class: "size-10 stroke-primary",
                title: "Code Icon",
              }
            }
            h3 { class: "text-2xl font-bold", "Interoperability" }
          }
          ul { class: "space-y-4",
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-primary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdX,
                  class: "stroke-primary",
                  title: "X mark",
                }
              }
              span { "No specific firmware" }
            }
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-primary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdX,
                  class: "stroke-primary",
                  title: "X mark",
                }
              }
              span { "No supported device list" }
            }
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-primary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-primary",
                  title: "Check mark",
                }
              }
              span { "Just a simple API" }
            }
          }
        }
        div {
          class: "card card-border space-y-8 justify-around bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
          style: "animation-delay: 0.2s;",
          div { class: "card-title space-x-4",
            div { class: "p-2 rounded-2xl bg-secondary-content flex items-center justify-center feature-icon shadow-lg",
              Icon {
                icon: LdLockKeyhole,
                class: "size-10 stroke-secondary",
                title: "Lock Icon",
              }
            }
            h3 { class: "text-2xl font-bold", "Security" }
          }
          ul { class: "space-y-4",
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-secondary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-secondary",
                  title: "Check mark",
                }
              }
              span { "100% written in Rust" }
            }
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-secondary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-secondary",
                  title: "Check mark",
                }
              }
              span { "Encryption required" }
            }
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-secondary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-secondary",
                  title: "Check mark",
                }
              }
              span { "Credentials secured with Ory stack" }
            }
          }
        }
        div {
          class: "card card-border space-y-8 justify-around bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
          style: "animation-delay: 0.2s;",
          div { class: "card-title space-x-4",
            div { class: "p-2 rounded-2xl bg-primary-content flex items-center justify-center feature-icon shadow-lg",
              Icon {
                icon: LdCopyleft,
                class: "size-10 stroke-primary",
                title: "Copyleft icon",
              }
            }
            h3 { class: "text-2xl font-bold", "Open Source" }
          }
          ul { class: "space-y-4",
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-primary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-primary",
                  title: "Check mark",
                }
              }
              span { "AGPL-3.0 Licensed" }
            }
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-primary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-primary",
                  title: "Check mark",
                }
              }
              span { "Built on open source tools" }
            }
            li { class: "flex items-center",
              div { class: "size-6 rounded-full bg-primary-content flex items-center justify-center mr-3",
                Icon {
                  icon: LdCheck,
                  class: "stroke-primary",
                  title: "Check mark",
                }
              }
              span { "Comprehensive documentation" }
            }
          }
        }
      }
    }
  }
}
