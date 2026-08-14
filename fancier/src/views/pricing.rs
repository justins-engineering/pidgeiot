use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdCheck, LdPlay};

/// One rung of the ladder. Every paid tier carries a "planned" badge and a
/// price note saying it is not billing, because publishing numbers we do not
/// charge yet is only honest if that is impossible to miss.
#[component]
fn TierCard(
  name: String,
  badge: String,
  tagline: String,
  price: String,
  cadence: String,
  price_note: String,
  features: Vec<String>,
  cta: Element,
  featured: bool,
) -> Element {
  let frame = if featured {
    "border-primary shadow-xl"
  } else {
    "border-base-300"
  };

  rsx! {
    div { class: "flex flex-col rounded-2xl border {frame} bg-base-100 p-6 h-full",
      div { class: "flex items-baseline gap-2 flex-wrap mb-1",
        h2 { class: "text-xl font-bold", "{name}" }
        if !badge.is_empty() {
          span { class: "badge badge-sm badge-outline font-mono tracking-wide", "{badge}" }
        }
      }
      p { class: "text-sm text-base-content/70 mb-5", "{tagline}" }

      div { class: "flex items-baseline gap-1",
        span { class: "text-4xl font-extrabold tracking-tight", "{price}" }
        if !cadence.is_empty() {
          span { class: "text-base-content/60", "{cadence}" }
        }
      }
      p { class: "mt-1 mb-6 text-xs text-base-content/60", "{price_note}" }

      ul { class: "flex flex-col gap-2 text-sm mb-6",
        for line in features.iter() {
          li { class: "flex gap-2 items-start",
            Icon {
              icon: LdCheck,
              class: "w-4 h-4 mt-0.5 shrink-0 stroke-primary",
              title: "Included",
            }
            span { "{line}" }
          }
        }
      }

      div { class: "mt-auto", {cta} }
    }
  }
}

/// A cost that never appears on an invoice. Separated from the ladder
/// because "what we will not charge for" is the part a fleet operator is
/// actually scanning for.
#[component]
fn NeverCard(label: String, value: String, note: String, body: String) -> Element {
  rsx! {
    div { class: "rounded-2xl border border-base-300 bg-base-100 p-6",
      p { class: "font-mono text-xs tracking-widest uppercase text-primary mb-3", "{label}" }
      div { class: "flex items-baseline gap-2 flex-wrap",
        span { class: "text-2xl font-bold", "{value}" }
        span { class: "text-xs text-base-content/60", "{note}" }
      }
      p { class: "mt-3 text-sm text-base-content/70 leading-relaxed", "{body}" }
    }
  }
}

#[component]
fn Answer(question: String, body: String) -> Element {
  rsx! {
    div {
      h3 { class: "text-lg font-bold mb-2", "{question}" }
      p { class: "text-base-content/75 leading-relaxed", "{body}" }
    }
  }
}

#[component]
pub fn PricingPage() -> Element {
  rsx! {
    section { class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Pricing" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "Free while we're in beta. Here's what we plan to charge after."
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-3xl text-base-content/80 text-pretty",
          "We're pre-revenue and won't pretend otherwise — nothing below is billing today. One ladder, no editions, no feature paywall: device count is the only number you'd have to forecast."
        }
      }
    }

    section { class: "px-4 md:px-10 py-14",
      div { class: "max-w-6xl mx-auto",
        div { class: "grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-6",

          TierCard {
            name: "Perch",
            badge: "not billing yet",
            tagline: "A real pilot, not a teaser.",
            price: "$0",
            cadence: "",
            price_note: "free in beta, and after · no card",
            features: vec![
                "10 devices".into(),
                "300K pooled messages/mo".into(),
                "7 days of history".into(),
                "1 seat · 1 alert".into(),
            ],
            featured: true,
            cta: rsx! {
              Link {
                class: "btn btn-primary w-full font-bold",
                to: Route::RegisterFlow { flow: None },
                "Start free"
              }
            },
          }

          TierCard {
            name: "Builder",
            badge: "planned",
            tagline: "First hardware out the door.",
            price: "$29",
            cadence: "/mo",
            price_note: "$0.55 per extra device · not billing yet",
            features: vec![
                "50 devices".into(),
                "1.5M pooled messages/mo".into(),
                "90 days of history".into(),
                "3 seats · 10 alerts · 1 org".into(),
                "Telemetry forwarding to your own store".into(),
            ],
            featured: false,
            cta: rsx! {
              div { class: "btn btn-outline w-full font-bold btn-disabled", "Free in beta" }
            },
          }

          TierCard {
            name: "Growth",
            badge: "planned",
            tagline: "A fleet with customers on it.",
            price: "$99",
            cadence: "/mo",
            price_note: "$0.35 per extra device · not billing yet",
            features: vec![
                "250 devices".into(),
                "7.5M pooled messages/mo".into(),
                "180 days of history".into(),
                "Unlimited seats, orgs and alerts".into(),
                "Priority email support".into(),
            ],
            featured: false,
            cta: rsx! {
              div { class: "btn btn-outline w-full font-bold btn-disabled", "Free in beta" }
            },
          }

          TierCard {
            name: "Scale",
            badge: "planned",
            tagline: "Thousands in the field.",
            price: "$349",
            cadence: "/mo",
            price_note: "$0.20 per extra device · not billing yet",
            features: vec![
                "1,500 devices".into(),
                "45M pooled messages/mo".into(),
                "12 months of history".into(),
                "Unlimited seats, orgs and alerts".into(),
                "SSO · priority support with SLA".into(),
            ],
            featured: false,
            cta: rsx! {
              div { class: "btn btn-outline w-full font-bold btn-disabled", "Free in beta" }
            },
          }
        }

        // Fleet sits outside the grid rather than as a fifth column: the
        // design's own copy says it should be scoped in a conversation, and
        // a card row invites comparison shopping instead.
        div { class: "mt-6 rounded-2xl border border-base-300 bg-base-100 p-6 md:p-8 flex flex-col lg:flex-row gap-6 lg:items-center",
          div { class: "lg:max-w-2xl",
            div { class: "flex items-baseline gap-2 flex-wrap mb-2",
              h2 { class: "text-xl font-bold", "Fleet" }
              span { class: "badge badge-sm badge-outline font-mono tracking-wide",
                "planned · talk to us first"
              }
            }
            p { class: "text-base-content/75 leading-relaxed",
              "10,000 devices, 300M pooled messages, $0.12 per device beyond. We'd rather scope a fleet this size with you than sell it from a page — MQTT and custom dashboards aren't here yet, and you should hear that from us before you sign."
            }
          }
          div { class: "lg:ml-auto lg:text-right shrink-0",
            div { class: "flex items-baseline gap-1 lg:justify-end",
              span { class: "text-4xl font-extrabold tracking-tight", "$1,499" }
              span { class: "text-base-content/60", "/mo" }
            }
            p { class: "mt-1 mb-4 text-xs text-base-content/60", "indicative · not billing yet" }
            a { class: "btn btn-outline font-bold", href: "mailto:code@jes.contact", "Talk to us" }
          }
        }
      }
    }

    section { class: "px-4 md:px-10 pb-14",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 md:grid-cols-3 gap-6",
        NeverCard {
          label: "Not billing yet",
          value: "$0.30",
          note: "per 10,000 · planned rate",
          body: "A billable message is a device→platform report: telemetry, shadow report-back, or a log upload. Pooled across the whole account.",
        }
        NeverCard {
          label: "Never metered",
          value: "Firmware bandwidth",
          note: "",
          body: "OTA is free at any image size on every tier, including free. So are shadow polls, dashboard API calls and provisioning.",
        }
        NeverCard {
          label: "Never billed",
          value: "Idle devices",
          note: "",
          body: "You're charged for devices that connected at least once in the month. The 400 units sitting in a warehouse are free.",
        }
      }
    }

    section { class: "px-4 md:px-10 py-14 bg-base-200 border-y border-base-300",
      div { class: "max-w-4xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight",
          "1,000 devices, reporting every five minutes"
        }
        p { class: "mt-3 text-base-content/70",
          "Published list prices, checked 12 Aug 2026. Cheapest tier that legitimately fits the fleet."
        }
        div { class: "mt-6 overflow-x-auto rounded-2xl border border-base-300 bg-base-100",
          table { class: "table",
            thead {
              tr {
                th { "Platform" }
                th { class: "text-right", "Per device" }
                th { class: "text-right", "monthly, USD" }
              }
            }
            tbody {
              tr { class: "font-bold",
                td { "PidgeIoT · Scale" }
                td { class: "text-right", "$0.35" }
                td { class: "text-right", "$349" }
              }
              tr {
                td { "ThingsBoard Cloud · Business" }
                td { class: "text-right", "$0.75" }
                td { class: "text-right", "$749" }
              }
              tr {
                td { "Particle · Basic blocks" }
                td { class: "text-right", "$3.89" }
                td { class: "text-right", "$3,887" }
              }
              tr {
                td { "Blues · Essentials" }
                td { class: "text-right", "$6.60" }
                td { class: "text-right", "$6,604" }
              }
            }
          }
        }
        p { class: "mt-5 text-base-content/70 leading-relaxed",
          "We won't claim to be cheaper than raw AWS or Azure — nobody is. We're cheaper than "
          span { class: "italic", "building on" }
          " them, because a message bus isn't a device list, a shadow editor, graphs, a log viewer, OTA orchestration and alerting. Ask us for the line-item comparison; we'll send the arithmetic."
        }
      }
    }

    section { class: "px-4 md:px-10 py-14",
      div { class: "max-w-4xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight mb-8", "Straight answers" }
        div { class: "flex flex-col gap-8",
          Answer {
            question: "What counts as a message?",
            body: "A report from a device to us. Shadow polls, firmware chunks, dashboard calls and WebSocket keep-alives don't count — they'd punish exactly the behaviour we want to encourage.",
          }
          Answer {
            question: "What happens if I go over?",
            body: "Nothing is billed in beta, and nothing is metered yet either. When paid tiers start, overage will run at $0.30 per 10,000 and service will keep going; free accounts will pause ingestion instead, warned well before the cap — no surprise invoice, ever.",
          }
          Answer {
            question: "Is anything locked behind a tier?",
            body: "No feature that costs us nothing to serve. Every transport, OTA, remote logs, the firmware catalog and per-device crypto are in the free tier. Tiers differ by devices, messages, retention, SSO and support.",
          }
          Answer {
            question: "Can I self-host it?",
            body: "Honestly: not usefully. The backend is built on Cloudflare Workers and Durable Objects, so \"self-hosting\" means running your own Cloudflare account. The source is public and always will be — but we're not going to sell you a self-host SKU we can't support well.",
          }
          Answer {
            question: "Then what stops lock-in?",
            body: "The exit, not the licence. The device library, the wire protocol and the API spec are open and documented forever, telemetry forwards to any line-protocol store you own, and every device's history is readable straight from the documented API.",
          }
          Answer {
            question: "Will these prices hold?",
            body: "These are planned prices, published early so you can budget — deliberately introductory while MQTT and custom dashboards are still missing. They can still move before billing starts, and we'll tell you well ahead of any change that affects you.",
          }
        }
      }
    }

    section { class: "px-4 md:px-10 pb-24",
      div { class: "max-w-4xl mx-auto rounded-3xl border border-neutral-content bg-linear-to-br/srgb from-primary/40 via-secondary/40 to-accent/40 p-10 text-center shadow-2xl",
        h2 { class: "text-2xl md:text-3xl font-bold mb-3", "Everything's free while we're in beta." }
        p { class: "text-lg mb-8 leading-relaxed",
          "Ten devices stay free after that. Start now and help shape what the rest costs."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-4",
          Link {
            class: "btn btn-lg btn-glow font-bold",
            to: Route::RegisterFlow { flow: None },
            Icon { icon: LdPlay, class: "mr-2", title: "Start free" }
            "Start free"
          }
          Link { class: "btn btn-lg btn-outline font-bold", to: Route::DemoPage {},
            "Try the live demo"
          }
        }
      }
    }
  }
}
