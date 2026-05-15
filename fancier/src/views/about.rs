use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCloud, LdLock, LdLockOpen, LdNetwork, LdUsers, LdWrench,
};

#[component]
pub fn AboutUs() -> Element {
  rsx! {
    // Hero Section: The Belief
    section {
      aria_label: "Introduction to our mission",
      class: "py-32 md:py-48 flex flex-col items-center text-center",

      div { class: "max-w-5xl",
        Icon {
          icon: LdNetwork,
          class: "w-12 h-12 mx-auto mb-10",
          title: "Abstract network nodes connecting",
        }
        h1 { class: "text-5xl md:text-7xl font-extrabold tracking-tighter mb-8 text-balance",
          "Infrastructure Should Empower. Not Restrict."
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-3xl mx-auto text-balance",
          "We started PidgeIoT because we were tired of walled gardens. When you build something, you should truly own it."
        }
      }
    }

    // The Origin Story (The "Why")
    section {
      aria_label: "Our origin story",
      class: "py-24 md:py-32 bg-base-200/50",

      div { class: "max-w-7xl mx-auto px-4 md:px-8 flex flex-col lg:flex-row gap-20 items-center",
        // Left: Narrative
        div { class: "lg:w-1/2",
          h2 { class: "text-3xl md:text-4xl font-bold mb-8 tracking-tight",
            "Born from Real-World Friction"
          }
          div { class: "space-y-6 text-lg text-base-content/70 leading-relaxed",
            p {
              "This project wasn't dreamed up in a vacuum. It was forged out of sheer necessity. We were in the trenches, tasked with deploying and maintaining complex digital signage networks across sprawling public transit systems."
            }
            p {
              "We hit the same walls every hardware team hits: platforms were too rigid, scaling was prohibitively expensive, and we were forced into proprietary connectivity boxes. We spent more time fighting our infrastructure than improving our product."
            }
            p { class: "font-semibold text-base-content text-xl pt-4",
              "We realized the industry didn't need another platform. It needed an open foundation."
            }
          }
        }

        // Right: Minimalist Visual Stacking
        div { class: "lg:w-1/2 w-full flex flex-col gap-6",
          article { class: "p-8 rounded-2xl bg-base-100 flex gap-6 items-start opacity-70",
            div { class: "mt-1",
              Icon {
                icon: LdLock,
                class: "w-6 h-6",
                title: "Locked padlock",
              }
            }
            div {
              h3 { class: "text-xl font-bold line-through decoration-2 mb-2",
                "Vendor Lock-in"
              }
              p { class: "text-base-content",
                "Trapped in ecosystems that dictated how and where we could scale."
              }
            }
          }

          article { class: "p-8 rounded-2xl bg-primary/10 flex gap-6 items-start transform transition hover:scale-[1.02]",
            div { class: "mt-1",
              Icon {
                icon: LdLockOpen,
                class: "w-6 h-6 text-primary",
                title: "Unlocked padlock",
              }
            }
            div {
              h3 { class: "text-xl font-bold text-primary mb-2", "Open Control" }
              p { class: "text-base-content/70",
                "The catalyst for PidgeIoT: reclaiming ownership of our own deployments and hardware."
              }
            }
          }
        }
      }
    }

    // Who We Are (The "Who")
    section { aria_label: "The team archetypes", class: "py-32 bg-base-100/50",

      div { class: "max-w-7xl mx-auto px-4 md:px-8",
        div { class: "mb-20 max-w-2xl",
          h2 { class: "text-3xl md:text-4xl font-bold mb-6 tracking-tight",
            "The Minds Behind the Mission"
          }
          p { class: "text-xl text-base-content/70 leading-relaxed",
            "Building a unified layer for both software and cellular connectivity requires a village. Our team brings together deep, complementary expertise."
          }
        }

        // Minimalist Grid Layout
        div { class: "grid grid-cols-1 md:grid-cols-3 gap-12",

          // Persona 1
          article { class: "group flex flex-col",
            div { class: "mb-6 p-4 rounded-xl bg-base-200/50 inline-flex w-fit group-hover:bg-base-300/50 transition-colors",
              Icon {
                icon: LdWrench,
                class: "w-8 h-8 text-base-content",
                title: "Wrench tool",
              }
            }
            h3 { class: "text-2xl font-bold mb-4", "The Field Engineers" }
            p { class: "text-base-content/70 leading-relaxed",
              "The ones who actually deploy the hardware. We know what it takes to keep a fleet online when it's subjected to the elements, power cycles, and the chaos of the real world."
            }
          }

          // Persona 2
          article { class: "group flex flex-col",
            div { class: "mb-6 p-4 rounded-xl bg-base-200/50 inline-flex w-fit group-hover:bg-base-300/50 transition-colors",
              Icon {
                icon: LdCloud,
                class: "w-8 h-8 text-base-content",
                title: "Cloud infrastructure",
              }
            }
            h3 { class: "text-2xl font-bold mb-4", "The Cloud Architects" }
            p { class: "text-base-content/70 leading-relaxed",
              "Veterans of massive enterprise scaling. We bring the rigorous security, routing, and architectural standards expected by the world's largest tech companies directly into our open-source ecosystem."
            }
          }

          // Persona 3
          article { class: "group flex flex-col",
            div { class: "mb-6 p-4 rounded-xl bg-base-200/50 inline-flex w-fit group-hover:bg-base-300/50 transition-colors",
              Icon {
                icon: LdUsers,
                class: "w-8 h-8 text-base-content",
                title: "Group of users",
              }
            }
            h3 { class: "text-2xl font-bold mb-4", "The Ecosystem Builders" }
            p { class: "text-base-content/70 leading-relaxed",
              "Hardware is only as good as the community around it. We are deeply embedded in the open-source hardware space to ensure our tools play nicely with the boards you already love."
            }
          }
        }
      }
    }

    // Final Mission CTA
    section {
      aria_label: "Call to action to join the community",
      class: "py-32 bg-base-200/50 text-center",

      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        h2 { class: "text-4xl md:text-5xl font-bold mb-8 tracking-tight", "Build With Us" }
        p { class: "text-xl text-base-content/70 mb-12 leading-relaxed text-balance",
          "We are building PidgeIoT in the open because infrastructure of this scale belongs to the community. We aren't just looking for users; we are looking for collaborators who share our vision."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-6",
          Link {
            class: "btn btn-primary btn-lg px-10 rounded-full",
            to: Route::SignUp {},
            "View the Platform"
          }
          a {
            class: "btn btn-ghost btn-lg px-10 rounded-full border border-base-content/20 hover:border-base-content/40 hover:bg-transparent",
            href: "https://discord.gg/W2vjtpeP",
            "Connect with the Team"
          }
        }
      }
    }
  }
}
