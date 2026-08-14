use dioxus::prelude::*;

#[component]
pub fn TermsPage() -> Element {
  rsx! {
    section { id: "terms-of-service", class: "py-16 md:py-24",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        h1 { class: "text-4xl md:text-5xl font-extrabold tracking-tighter mb-3", "Terms of Service" }
        p { class: "text-sm text-base-content/50 mb-10", "Last updated: July 26, 2026" }

        p { class: "text-lg text-base-content/70 leading-relaxed",
          "These terms govern your use of PidgeIoT, operated by Justin's Engineering Services LLC, a Massachusetts limited liability company (\"we\", \"us\"). By creating an account or connecting a device, you agree to them. Questions: "
          a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
          "."
        }

        LegalSection { title: "Early access, as-is",
          p { class: "mb-4",
            "PidgeIoT is in early access. The service is provided \"as is\" and \"as available\", without warranties of any kind, express or implied. Features may change, break, or be removed as the platform develops, and we do not guarantee uninterrupted availability or that stored data will never be lost. Keep your own copies of anything you cannot afford to lose."
          }
          p {
            "We work hard to keep it up and to keep your data intact — but during beta you should treat this as a developing platform, not a finished product."
          }
        }

        LegalSection { title: "Pricing",
          p {
            "PidgeIoT is currently free to use. We expect to introduce paid tiers in the future; if pricing changes in a way that affects your account, we will give you notice before it takes effect."
          }
        }

        LegalSection { title: "Your account",
          p {
            "You are responsible for the credentials and device tokens issued to your account, and for the activity of devices provisioned under it. Keep device tokens secret; anyone holding a valid token can act as that device."
          }
        }

        LegalSection { title: "Acceptable use",
          p { class: "mb-4", "You agree not to:" }
          ul { class: "list-disc ml-6 space-y-2",
            li { "Use the service for anything illegal." }
            li { "Use alert emails or any other platform feature to send spam or abusive messages to others." }
            li {
              "Attempt to access other users' accounts, devices, or data, or to probe, bypass, or interfere with the platform's security or other users' use of it."
            }
          }
          p { class: "mt-4",
            "We may suspend or terminate accounts that violate these terms, with or without prior warning where the abuse is ongoing."
          }
        }

        LegalSection { title: "Your data",
          p {
            "Data you and your devices send to the platform remains yours. How we handle it is described in the "
            Link { class: "link link-secondary", to: crate::Route::PrivacyPage {}, "Privacy Policy" }
            "."
          }
        }

        LegalSection { title: "Limitation of liability",
          p {
            "To the maximum extent permitted by law, Justin's Engineering Services LLC will not be liable for any indirect, incidental, special, consequential, or exemplary damages — including lost profits, lost data, or business interruption — arising from your use of, or inability to use, the service. In particular, do not rely on a beta service as the sole safety mechanism for anything where failure could cause harm."
          }
        }

        LegalSection { title: "Governing law",
          p {
            "These terms are governed by the laws of the Commonwealth of Massachusetts, without regard to its conflict-of-law rules."
          }
        }

        LegalSection { title: "Changes to these terms",
          p {
            "We may update these terms as the service evolves. Changes will be posted on this page with a revised \"Last updated\" date, and material changes will be flagged with notice via the site. Continuing to use the service after a change takes effect means you accept the updated terms."
          }
        }
      }
    }
  }
}

#[component]
fn LegalSection(title: &'static str, children: Element) -> Element {
  rsx! {
    div { class: "mt-12 text-base-content/70 leading-relaxed",
      h2 { class: "text-2xl md:text-3xl font-bold mb-4 tracking-tight text-base-content", "{title}" }
      {children}
    }
  }
}
