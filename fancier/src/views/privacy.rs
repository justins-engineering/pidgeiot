use capsules::PRIVACY_NOTICE_VERSION;
use dioxus::prelude::*;

#[component]
pub fn PrivacyPage() -> Element {
  rsx! {
    section { id: "privacy-policy", class: "py-16 md:py-24",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        h1 { class: "text-4xl md:text-5xl font-extrabold tracking-tighter mb-3", "Privacy Policy" }
        p { class: "text-sm text-base-content/50 mb-10", "Last updated: {PRIVACY_NOTICE_VERSION}" }

        p { class: "text-lg text-base-content/70 leading-relaxed mb-4",
          "This policy describes what the PidgeIoT platform collects, where it lives, how long we keep it, and what we do (and deliberately don't do) with it."
        }
        p { class: "text-lg text-base-content/70 leading-relaxed",
          "Questions about anything here: "
          a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
          "."
        }

        LegalSection { id: "privacy-controller", title: "Who is responsible for your data",
          p { class: "mb-4",
            "PidgeIoT is operated by Justin's Engineering Services LLC, a Massachusetts limited liability company. For the account you create, the messages you send us, and the diagnostics your browser sends us, we are the controller of your personal data."
          }
          p {
            "For the data your devices and your team put into the platform, you (or the organization you belong to) are the controller and we process it on your instructions under our Data Processing Agreement, which any customer can countersign by emailing "
            a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
            "."
          }
        }

        LegalSection { id: "privacy-collect", title: "What we collect",
          p { class: "mb-4",
            "Account data. When you register a dashboard account, our self-hosted Ory Kratos identity system stores your email address and a hash of your password. We never store your password in plain text."
          }
          p { class: "mb-4",
            "Device data. The platform exists to hold the data your devices send it: telemetry values, device configuration (shadow state), and device log uploads, along with the metadata you enter when creating flocks and pigeons (names, descriptions, connector settings). You control what your devices report."
          }
          p { class: "mb-4",
            "Web logs. Like nearly every web service, our infrastructure records standard request logs (IP address, user agent, timestamps, and the routes requested) used for debugging and abuse prevention."
          }
          p {
            "Error diagnostics. If the dashboard hits a bug, your browser sends us a technical report: the error message, the place in our code where it happened, the app build, the page's route template, your browser's user agent string, and a short trail of recent in-app actions recorded as request method, route template, and status code. These reports are anonymous by design. They carry no account identity, no full URLs, no query strings, no form contents, and no request or response bodies, and we do not link them to your session. If you choose to send us a problem report yourself, we attach your account identity to that report so we can follow up with you, and identified reports are deleted with your account. Error reports are kept for 90 days; the long-lived statistics we keep about error patterns contain no personal data."
          }
        }

        LegalSection { id: "privacy-not-doing", title: "What we don't do",
          ul { class: "list-disc ml-6 space-y-2",
            li { "We do not sell your data. Not account data, not telemetry, not anything." }
            li { "We do not run third-party advertising or ad-tracking scripts on this site." }
            li {
              "We do not use cookies, browser storage or analytics to profile you or to follow you to other sites. The next section lists everything we do set, and why."
            }
          }
        }

        LegalSection { id: "privacy-cookies", title: "Cookies, storage and analytics",
          p { class: "mb-4",
            "This is the complete list of what the site stores in your browser or reads back from it. None of it is used for advertising or for cross-site tracking, and nothing on this list is sold or shared with anyone for their own purposes."
          }
          ul { class: "space-y-3 mb-6",
            StorageItem {
              name: "ory_kratos_session",
              kind: "cookie",
              "Keeps you signed in. Set by our authentication service on auth.pidgeiot.com and scoped to pidgeiot.com so the dashboard and the API both see it. Marked HttpOnly, so no script can read it. A session lasts 4 hours."
            }
            StorageItem {
              name: "csrf_token_<hash>",
              kind: "cookie",
              "Protects the sign-in, registration, recovery and account settings forms against cross-site request forgery. Set by the same authentication service, one per form, with Domain=pidgeiot.com, HttpOnly and Secure, and a one-year lifetime. That lifetime is the identity software's own default and outlives by far the session it protects."
            }
            StorageItem {
              name: "session_expiry",
              kind: "cookie",
              "Our own hint of when your session ends, so the dashboard can sign you out on time without asking the network. Its value is a timestamp and nothing else: no identifier, no account, no token. It is deliberately readable by this page's script, because the sign-in cookie above is not."
            }
            StorageItem {
              name: "theme",
              kind: "browser storage",
              "Remembers whether you chose the light or the dark theme. Written when you click the toggle."
            }
            StorageItem {
              name: "pidgeiot.graphs.v1.*",
              kind: "browser storage",
              "The telemetry graphs you configure for a pigeon or a flock. This is your own content, it never leaves your browser, and it exists only behind the sign-in."
            }
            StorageItem {
              name: "pidgeiot.return_to.v1",
              kind: "browser storage",
              "The page you were on when a sign-in interrupted you, so we can put you back there afterwards. Kept for 30 minutes at most, and deleted the moment it is read."
            }
            StorageItem {
              name: "Cloudflare Turnstile",
              kind: "third-party script, contact page only",
              "Anti-abuse on the public contact form. It loads from challenges.cloudflare.com after the page has rendered, and on no other page of this site."
            }
          }
          p { class: "mb-4",
            "Analytics. We use Cloudflare Web Analytics to count page views on our public pages. It is not served to visitors connecting from the European Economic Area, the United Kingdom or Switzerland, so if you are in one of those places no analytics script runs in your browser at all. For everyone else it sets no cookies, stores nothing on your device, and does not identify you or follow you to other websites; what it records is the page you viewed, the site that linked you to it, your browser, operating system and device type, the country your connection came from, and how quickly the page loaded."
          }
          p {
            "Separately from that script, and whether or not it runs, our edge provider records the request itself: the standard web log described under \"What we collect\", kept for the period given in the retention table below."
          }
        }

        LegalSection { id: "privacy-tracking-signals",
          title: "Do Not Track and Global Privacy Control",
          p {
            "Some browsers send a Global Privacy Control or Do Not Track signal on your behalf. There is nothing here for either signal to switch off: we sell no personal data and share none for cross-context advertising, we serve no advertising and no cross-site tracking, and the one analytics script we run is not served at all to visitors in the European Economic Area, the United Kingdom or Switzerland. If that ever changes, honoring the signal becomes something we have to build rather than something we can simply state, and this section will say so."
          }
        }

        LegalSection { id: "privacy-transfers",
          title: "Where your data is processed, and how transfers are protected",
          p { class: "mb-4",
            "We are a United States company, and the platform runs on infrastructure in the United States and on a global edge network. All traffic between your browser or your devices and the platform is encrypted in transit with TLS. In plain terms:"
          }
          ul { class: "list-disc ml-6 space-y-2 mb-4",
            li {
              "Each device's own state (its configuration, its latest readings and its log buffer) lives in a Cloudflare Durable Object that is created near whoever first set the device up, and stays there. For a team in Europe that is usually a European data center, but we do not guarantee it."
            }
            li { "Our relational database and our identity database are hosted by Crunchy Bridge on AWS in Northern Virginia (us-east-1)." }
            li { "Our identity server and our device-transport terminator run on a server in Vint Hill, Virginia." }
            li { "Our edge provider runs our code in whichever of its data centers receives a request, and its queues and caches have no fixed location." }
            li { "Billing is handled by Stripe in the United States. Transactional email is sent by a third-party email provider." }
          }
          p { class: "mb-4",
            "If you are in the European Economic Area, the United Kingdom or Switzerland, this means your personal data is transferred to the United States. We rely on the European Commission's Standard Contractual Clauses (Commission Implementing Decision (EU) 2021/914 of 4 June 2021, Module Two), together with the UK International Data Transfer Addendum for UK data and the Swiss adaptations for Swiss data, as the legal basis for that transfer. Those clauses are part of our Data Processing Agreement. We are not certified under the EU-U.S. Data Privacy Framework; some of our service providers are, and we rely on their certification for the part of the processing they do."
          }
          p { class: "mb-4",
            "We do not offer EU data residency today. If you need it, contact us and tell us the requirement."
          }
          p {
            "Device credentials are handled asymmetrically: only a device's public key is ever persisted. The platform cannot recover a device token after it is first shown to you."
          }
        }

        LegalSection { id: "privacy-subprocessors", title: "Service providers we use",
          p {
            "We use a small number of service providers to run the platform. The current list, with what each one does, where it processes data, and the transfer safeguard that covers it, forms part of our Data Processing Agreement and is available on request from "
            a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
            ". We give customers thirty days' notice by email before we add or replace one."
          }
        }

        LegalSection { id: "privacy-retention", title: "How long we keep data",
          p { class: "mb-4",
            "We keep data for as long as it serves the purpose it was collected for, and no longer. The concrete periods are:"
          }
          div { class: "overflow-x-auto",
            // Fixed layout because the auto one asks for 20px more than a
            // 390px phone has, and a two-column table of sentences is worth
            // reading without nudging every row sideways.
            table { class: "table table-sm table-fixed w-full",
              thead {
                tr {
                  th { class: "align-top w-2/5 md:w-1/3 whitespace-normal", "Data" }
                  th { class: "align-top whitespace-normal", "How long, and what happens then" }
                }
              }
              tbody {
                RetentionRow {
                  data: "Your account (email, name, phone if you give one, credentials)",
                  period: "While your account exists. Deleted when you ask us to delete it.",
                }
                RetentionRow { data: "Sign-in sessions", period: "4 hours, then they expire." }
                RetentionRow {
                  data: "Verification and recovery codes",
                  period: "Minutes to hours, and single use. The record that the message was sent stays in the identity system's own log.",
                }
                RetentionRow {
                  data: "Organization invitations",
                  period: "7 days, and single use, then they expire.",
                }
                RetentionRow {
                  data: "Device configuration, latest readings, device log buffer",
                  period: "While the device exists, and the log buffer keeps only the newest 200 chunks. Erased when you delete the device.",
                }
                RetentionRow {
                  data: "Telemetry history",
                  period: "7 days on the free tier, 30 days on Builder, 90 days on Growth, 13 months on Scale and Fleet. Deleted automatically after that.",
                }
                RetentionRow {
                  data: "Firmware images",
                  period: "While the fleet exists. Removed by us on request.",
                }
                RetentionRow {
                  data: "Billing records (invoices, subscription history)",
                  period: "As long as tax and accounting law require, held by our payment processor. Deleted at the end of the statutory period.",
                }
                RetentionRow {
                  data: "Contact-form and support messages",
                  period: "Kept as correspondence you addressed to us. Deleting your account detaches your account identifier from the message rather than deleting the message itself.",
                }
                RetentionRow {
                  data: "Dashboard error reports",
                  period: "90 days, then deleted automatically. The statistics we keep about error patterns contain no personal data.",
                }
                RetentionRow {
                  data: "Web and API request logs",
                  period: "7 days, then deleted automatically by our edge provider.",
                }
                RetentionRow {
                  data: "Backups of our databases",
                  period: "Rotated on our database host's own schedule. Deleted data disappears from a backup when that backup expires.",
                }
              }
            }
          }
        }

        LegalSection { id: "privacy-legal-bases", title: "Why we are allowed to process your data",
          p { class: "mb-4",
            "If you are in the EEA, the UK or Switzerland, the law requires us to tell you the legal basis for each kind of processing:"
          }
          ul { class: "list-disc ml-6 space-y-2",
            li {
              strong { "To provide the service you signed up for" }
              " (creating and securing your account, running your devices, sending you the alerts you configure, billing your organization): performance of a contract (GDPR Article 6(1)(b))."
            }
            li {
              strong { "To keep tax and accounting records" }
              ", including validating an EU VAT number you give us against the European Commission's VIES register: a legal obligation (Article 6(1)(c))."
            }
            li {
              strong { "To keep the platform secure and working" }
              " (request logs, rate limiting, anonymous error diagnostics, notifying ourselves of failures): our legitimate interest in running a secure service (Article 6(1)(f)). We have designed these to carry as little personal data as possible; error reports carry no identity unless you choose to attach one."
            }
            li {
              strong { "To answer your messages" }
              " when you use the contact form or send feedback: our legitimate interest in responding to you, and, where you are asking about becoming a customer, steps you ask us to take before a contract (Article 6(1)(b) and (f))."
            }
            li {
              strong { "Email updates" }
              ": only with your consent, which you can withdraw at any time (Article 6(1)(a)). We do not send marketing email today."
            }
          }
        }

        LegalSection { id: "privacy-marketing", title: "Product updates by email",
          p {
            "If you tick the box for product updates, we send you occasional email about PidgeIoT. We do that only because you asked us to, which in legal terms means we rely on your consent (GDPR Article 6(1)(a)), and you can withdraw it at any time in your account settings without giving a reason and without affecting anything else about your account. Withdrawing takes effect for anything we have not already sent. We do not send this email unless you have asked for it, we do not share your address with anyone else for their own marketing, and every message we send includes a link to stop them."
          }
        }

        LegalSection { id: "privacy-objection", title: "Objecting to how we use your data",
          p {
            "You can object at any time to our sending you marketing email, and we will stop; this is an absolute right and we do not weigh it against anything (GDPR Article 21(2) and 21(3)). For the smaller number of things we do because we have a legitimate interest in them, such as keeping the service secure and diagnosing faults, you can also object, and we will stop unless we can show compelling grounds that override your interests. In either case, email us at the address in this notice, or use your account settings for the marketing choice. You do not have to explain why, and objecting costs you nothing."
          }
        }

        LegalSection { id: "privacy-rights", title: "Your rights",
          p { class: "mb-4",
            "If you are in the EEA, the UK or Switzerland, you have the right to ask us for access to the personal data we hold about you, to have it corrected or deleted, to restrict or object to how we process it, to receive it in a portable format, and, where we rely on consent, to withdraw that consent. You also have the right to complain to your data protection authority."
          }
          p { class: "mb-4", "Much of this you can do yourself:" }
          ul { class: "list-disc ml-6 space-y-2 mb-4",
            li {
              strong { "See and correct" }
              " your email, name and phone number in account settings."
            }
            li {
              strong { "Delete" }
              " devices, empty organizations, and your own identified error reports in the dashboard."
            }
            li {
              strong { "Take your data with you" }
              ": every fleet, device, configuration and telemetry history you can see in the dashboard is available as JSON through the API documented on our "
              Link { class: "link link-secondary", to: crate::Route::ApiReferencePage {}, "API reference" }
              " page, and you can configure a forwarding endpoint to receive your telemetry continuously."
            }
          }
          p { class: "mb-4",
            "For anything else, including deleting your account, email "
            a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
            " from the address on your account. We will confirm receipt within five business days and answer within one month; if a request is complex we may take up to two further months and will tell you why. We do not charge for this unless a request is clearly unfounded or excessive."
          }
          p {
            "If your data reached us through a customer's use of the platform (for example your organization's account, or a device your employer operates), that customer is the controller and we will pass your request to them."
          }
        }

        LegalSection { id: "privacy-deletion", title: "Deleting your data",
          p { class: "mb-4",
            "You can delete your pigeons and flocks directly in the dashboard at any time; deleting a pigeon removes its stored shadow, telemetry, and logs from the platform."
          }
          p {
            "There is no automated account-deletion flow yet. To delete your account, email "
            a { class: "link link-secondary", href: "mailto:info@jes.contact", "info@jes.contact" }
            " from your account's address and we will remove it."
          }
        }

        LegalSection { id: "privacy-automated-decisions", title: "Automated decisions",
          p {
            "We do not make decisions about you by automated means that have legal or similarly significant effects. Two automated checks exist and you should know about them: when an organization saves an EU VAT number we validate it against the European Commission's VIES register and will not accept a number the register says is invalid; and when a free-tier account exceeds its monthly message allowance, its devices' uploads are paused until the next period. Both are about the organization's account rather than about you as a person, and either can be raised with us by email."
          }
        }

        LegalSection { id: "privacy-forwarding", title: "Telemetry forwarding you configure",
          p {
            "PidgeIoT lets you configure a forwarding endpoint for a pigeon's telemetry. If you do, we send that pigeon's telemetry to the endpoint you configured instead of storing its history with us. That endpoint is chosen and controlled by you: data sent there is governed by whoever operates it, not by this policy."
          }
        }

        LegalSection { id: "privacy-email", title: "Email",
          p { class: "mb-4",
            "We send transactional email only: account verification, password recovery, and the alert notifications you configure. Delivery goes through a third-party SMTP provider, which necessarily processes the recipient address and message content in order to deliver it."
          }
          p { "We do not send marketing email today." }
        }

        LegalSection { id: "privacy-changes", title: "Changes to this policy",
          p {
            "As the platform evolves we may update this policy. Changes will be posted on this page with a revised \"Last updated\" date."
          }
        }
      }
    }
  }
}

#[component]
fn LegalSection(id: &'static str, title: &'static str, children: Element) -> Element {
  rsx! {
    section { id, class: "mt-12 text-base-content/70 leading-relaxed",
      h2 { class: "text-2xl md:text-3xl font-bold mb-4 tracking-tight text-base-content", "{title}" }
      {children}
    }
  }
}

/// One entry in the cookie and browser-storage inventory. A list rather than a
/// table because the descriptions are sentences, and a three-column table of
/// sentences is unreadable on a phone.
#[component]
fn StorageItem(name: &'static str, kind: &'static str, children: Element) -> Element {
  rsx! {
    li {
      div { class: "flex flex-wrap items-baseline gap-x-2",
        code { class: "font-mono text-sm break-all text-base-content", "{name}" }
        span { class: "text-xs uppercase tracking-wide text-base-content/50", "{kind}" }
      }
      div { class: "mt-1", {children} }
    }
  }
}

#[component]
fn RetentionRow(data: &'static str, period: &'static str) -> Element {
  rsx! {
    tr {
      th { class: "align-top font-medium text-base-content whitespace-normal", "{data}" }
      td { class: "align-top whitespace-normal", "{period}" }
    }
  }
}
