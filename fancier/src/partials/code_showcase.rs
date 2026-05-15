use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::fa_brands_icons::FaRust;
use dioxus_free_icons::icons::ld_icons::LdCheck;

#[component]
pub fn CodeShowcase() -> Element {
  rsx! {
    section { id: "showcase", class: "front-page",
      div { class: "flex flex-col lg:flex-row items-center gap-12",
        div { class: "lg:w-1/2 scroll-reveal",
          h2 { class: "text-3xl md:text-4xl font-bold mb-6",
            span { class: "gradient-text", "Memory Safety" }
            " Meets Performance"
          }
          p { class: "mb-8 text-lg leading-relaxed",
            "Experience the power of Rust's compile-time guarantees. Our platform leverages Rust's ownership system to eliminate entire classes of bugs and security vulnerabilities while delivering C++-level performance."
          }
          div { class: "space-y-4",
            div { class: "flex items-start",
              div { class: "w-8 h-8 rounded-full bg-primary-content flex items-center justify-center mr-4 mt-1",
                Icon {
                  icon: LdCheck,
                  class: "stroke-primary",
                  title: "Check Mark",
                }
              }
              div {
                h4 { class: "font-bold text-lg mb-1", "Zero Runtime Overhead" }
                p { "No garbage collector, no runtime. Pure performance." }
              }
            }
            div { class: "flex items-start",
              div { class: "w-8 h-8 rounded-full bg-secondary-content flex items-center justify-center mr-4 mt-1",
                Icon {
                  icon: LdCheck,
                  class: "stroke-secondary",
                  title: "Check Mark",
                }
              }
              div {
                h4 { class: "font-bold text-lg mb-1", "Fearless Concurrency" }
                p { "Write concurrent code without data races." }
              }
            }
            div { class: "flex items-start",
              div { class: "w-8 h-8 rounded-full bg-primary-content flex items-center justify-center mr-4 mt-1",
                Icon {
                  icon: LdCheck,
                  class: "stroke-primary",
                  title: "Check Mark",
                }
              }
              div {
                h4 { class: "font-bold text-lg mb-1", "Compile-Time Safety" }
                p { "Catch bugs before they reach production." }
              }
            }
          }
        }
        div { class: "max-w-full lg:w-1/2",
          div { class: "inline-flex items-center px-4 py-2 rounded-full bg-base-200 border border-base-content/30 mb-6",
            Icon {
              icon: FaRust,
              class: "text-orange-500 mr-2",
              title: "Rust Icon",
            }
            span { class: "text-sm font-medium", "Rust-Powered Excellence" }
          }
          div { class: "mockup-code",
            pre {
              code { class: "text-secondary", "use pigeon_iot::DeviceManager;" }
            }
            br {}
            pre {
              code { class: "text-accent-content dark:text-accent", "async fn " }
              code { class: "text-primary", "connect_device " }
              code { "(device_id: &str) -> Result<Device, Error> {{ " }
            }
            pre {
              "\n\t"
              code { class: "text-accent-content dark:text-accent", "let " }
              code { "mut manager = DeviceManager::new()" }
            }
            pre {
              "\t\t"
              code { ".with_security(SecurityLevel::Maximum)" }
            }
            pre {
              "\t\t"
              code { ".with_encryption(Encryption::AES256GCM)" }
            }
            pre {
              "\t\t"
              code { ".await?;" }
            }
            pre {
              "\n\t"
              code { class: "text-accent-content dark:text-accent", "let " }
              code { "device = manager.connect(device_id).await?;" }
            }
            pre {
              "\n\t"
              code { "Ok(device)" }
            }
            pre {
              code { "}}" }
            }
          }
        }
      }
    }
  }
}
