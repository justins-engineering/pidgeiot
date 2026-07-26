use dioxus::prelude::*;

#[component]
pub fn PrivacyPage() -> Element {
  rsx! {
    section { class: "py-16 md:py-24",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        h1 { class: "text-4xl md:text-5xl font-extrabold tracking-tighter mb-3", "Privacy Policy" }
        p { class: "text-sm text-base-content/50 mb-10", "Last updated: July 26, 2026" }

        p { class: "text-lg text-base-content/70 leading-relaxed mb-4",
          "PidgeIoT is operated by Justin's Engineering Services LLC, a Massachusetts limited liability company. This policy describes what data the platform collects, where it lives, and what we do — and deliberately don't do — with it."
        }
        p { class: "text-lg text-base-content/70 leading-relaxed",
          "Questions about anything here: "
          a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
          "."
        }

        LegalSection { title: "What we collect",
          p { class: "mb-4",
            "Account data. When you register a dashboard account, our self-hosted Ory Kratos identity system stores your email address and a hash of your password. We never store your password in plain text."
          }
          p { class: "mb-4",
            "Device data. The platform exists to hold the data your devices send it: telemetry values, device configuration (shadow state), and device log uploads, along with the metadata you enter when creating flocks and pigeons (names, descriptions, connector settings). You control what your devices report."
          }
          p {
            "Web logs. Like nearly every web service, our infrastructure records standard request logs — IP address, user agent, timestamps, and the routes requested — used for debugging and abuse prevention."
          }
        }

        LegalSection { title: "Where your data lives",
          p { class: "mb-4",
            "Account, device, and platform data are stored in managed PostgreSQL and on Cloudflare's edge infrastructure (Workers, Durable Objects, and object storage). All traffic between your browser or devices and the platform is encrypted in transit with TLS."
          }
          p {
            "Device credentials are handled asymmetrically: only a device's public key is ever persisted. The platform cannot recover a device token after it is first shown to you."
          }
        }

        LegalSection { title: "What we don't do",
          ul { class: "list-disc ml-6 space-y-2",
            li { "We do not sell your data. Not account data, not telemetry, not anything." }
            li { "We do not run third-party advertising or ad-tracking scripts on this site." }
            li { "We do not use tracking cookies. The only cookie we set is a session cookie, strictly for keeping you signed in." }
          }
        }

        LegalSection { title: "Telemetry forwarding you configure",
          p {
            "PidgeIoT lets you configure a forwarding endpoint for a pigeon's telemetry. If you do, we send that pigeon's telemetry to the endpoint you configured instead of storing its history with us. That endpoint is chosen and controlled by you — data sent there is governed by whoever operates it, not by this policy."
          }
        }

        LegalSection { title: "Email",
          p {
            "We send transactional email only: account verification, password recovery, and the alert notifications you configure. Delivery goes through a third-party SMTP provider, which necessarily processes the recipient address and message content in order to deliver it. We do not send marketing email."
          }
        }

        LegalSection { title: "Deleting your data",
          p { class: "mb-4",
            "You can delete your pigeons and flocks directly in the dashboard at any time; deleting a pigeon removes its stored shadow, telemetry, and logs from the platform."
          }
          p {
            "There is no automated account-deletion flow yet. To delete your account, email "
            a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
            " from your account's address and we will remove it."
          }
        }

        LegalSection { title: "Changes to this policy",
          p {
            "As the platform evolves we may update this policy. Changes will be posted on this page with a revised \"Last updated\" date."
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
