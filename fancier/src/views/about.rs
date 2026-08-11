use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCircuitBoard, LdCode, LdKeyRound, LdLock, LdNetwork, LdScale,
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
          "PidgeIoT is built so owning your fleet doesn't mean trusting a platform with it -- the backend, the dashboard, and the device firmware are all source you can read, fork, and run yourself."
        }
      }
    }

    // What This Actually Is
    section {
      aria_label: "What PidgeIoT actually is",
      class: "py-24 md:py-32 bg-base-200/50",

      div { class: "max-w-7xl mx-auto px-4 md:px-8 flex flex-col lg:flex-row gap-20 items-center",
        // Left: Plain facts, not narrative
        div { class: "lg:w-1/2",
          h2 { class: "text-3xl md:text-4xl font-bold mb-8 tracking-tight",
            "What PidgeIoT Actually Is"
          }
          div { class: "space-y-6 text-lg text-base-content/70 leading-relaxed",
            p {
              "PidgeIoT is built and run by Justin's Engineering Services, LLC -- a small, independent shop, not a venture-backed platform company. There's no enterprise tier gating the parts that matter; what's on this site is what exists."
            }
            p {
              "The stack is Rust end to end: the edge backend runs on Cloudflare Workers and Durable Objects, this dashboard compiles to WebAssembly, and both share the same Rust types so they can't quietly drift apart. Dashboard sign-in runs on a self-hosted Ory Kratos instance we operate ourselves, not a third-party identity vendor with our logo on it."
            }
            p { class: "font-semibold text-base-content text-xl pt-4",
              "It's in beta, it's free during beta, and it's licensed AGPL-3.0 from the backend to the device firmware -- none of that is something you have to take on faith."
            }
          }
        }

        // Right: A real design tradeoff, not a mood board
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
                "One Key For The Whole Fleet"
              }
              p { class: "text-base-content",
                "The failure mode this platform was designed around: a single shared credential baked into every device, impossible to revoke for just one."
              }
            }
          }

          article { class: "p-8 rounded-2xl bg-primary/10 flex gap-6 items-start transform transition hover:scale-[1.02]",
            div { class: "mt-1",
              Icon {
                icon: LdKeyRound,
                class: "w-6 h-6 text-primary",
                title: "Key",
              }
            }
            div {
              h3 { class: "text-xl font-bold text-primary mb-2", "A Keypair Per Device" }
              p { class: "text-base-content/70",
                "Every pigeon mints its own Ed25519 keypair in its own Durable Object. Refreshing one device's token can't touch any other device's, because there's no shared secret to leak in the first place."
              }
            }
          }
        }
      }
    }

    // Three Things We Can Prove
    section { aria_label: "What backs this platform's claims", class: "py-32 bg-base-100/50",

      div { class: "max-w-7xl mx-auto px-4 md:px-8",
        div { class: "mb-20 max-w-2xl",
          h2 { class: "text-3xl md:text-4xl font-bold mb-6 tracking-tight",
            "Three Things We Can Prove"
          }
          p { class: "text-xl text-base-content/70 leading-relaxed",
            "A fleet-management platform is asking you to trust it with real devices. Here's what backs that up -- not copy, just what's true right now."
          }
        }

        div { class: "grid grid-cols-1 md:grid-cols-3 gap-12",

          article { class: "group flex flex-col",
            div { class: "mb-6 p-4 rounded-xl bg-base-200/50 inline-flex w-fit group-hover:bg-base-300/50 transition-colors",
              Icon {
                icon: LdCircuitBoard,
                class: "w-8 h-8 text-base-content",
                title: "Circuit board",
              }
            }
            h3 { class: "text-2xl font-bold mb-4", "Verified on Real Hardware" }
            p { class: "text-base-content/70 leading-relaxed",
              "Provisioning, shadow sync, firmware updates, and WebSocket push have all been exercised on real boards -- ESP32-C6, and Nordic nRF9160 and nRF9151 -- not just in simulation."
            }
          }

          article { class: "group flex flex-col",
            div { class: "mb-6 p-4 rounded-xl bg-base-200/50 inline-flex w-fit group-hover:bg-base-300/50 transition-colors",
              Icon {
                icon: LdCode,
                class: "w-8 h-8 text-base-content",
                title: "Code brackets",
              }
            }
            h3 { class: "text-2xl font-bold mb-4", "No Proprietary Firmware" }
            p { class: "text-base-content/70 leading-relaxed",
              "The device library, pigeon, is a plain Zephyr RTOS module. There's no closed vendor blob between your firmware and the platform -- it's a dependency you can read like any other."
            }
          }

          article { class: "group flex flex-col",
            div { class: "mb-6 p-4 rounded-xl bg-base-200/50 inline-flex w-fit group-hover:bg-base-300/50 transition-colors",
              Icon {
                icon: LdScale,
                class: "w-8 h-8 text-base-content",
                title: "Balance scale",
              }
            }
            h3 { class: "text-2xl font-bold mb-4", "AGPL-3.0, Not \"Open-ish\"" }
            p { class: "text-base-content/70 leading-relaxed",
              "The backend, the dashboard, and the device library are all licensed AGPL-3.0 and developed in the open on GitHub. If a claim on this site doesn't check out, the source is right there to prove it."
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
          "PidgeIoT is free during beta and the source is public. The fastest way to evaluate it is to provision a pigeon or read the code -- not to read another page like this one."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-6",
          Link {
            class: "btn btn-primary btn-lg px-10 rounded-full",
            to: Route::RegisterFlow { flow: None },
            "Start Building, Free"
          }
          a {
            class: "btn btn-ghost btn-lg px-10 rounded-full border border-base-content/20 hover:border-base-content/40 hover:bg-transparent",
            href: "https://discord.gg/W2vjtpeP",
            "Join the Discord"
          }
        }
      }
    }
  }
}
