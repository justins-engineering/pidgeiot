use crate::components::{Maturity, MaturityBadge};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdBellRing, LdDatabase, LdHardDriveDownload, LdLineChart, LdMapPin, LdScrollText,
};

/// The "what you can actually do tonight" grid — six concrete capabilities,
/// each one reachable in the product today. Per the `MaturityBadge`
/// convention (`components/maturity_badge.rs`), a card with no badge is
/// production-ready; `Beta` marks real, verified code that hasn't been
/// promoted everywhere yet. Nothing design-stage belongs here at all —
/// that's the features page's roadmap section.
#[component]
pub fn Features() -> Element {
  rsx! {
    section { id: "features", class: "front-page",
      div { class: "text-center mb-4",
        h2 { class: "text-3xl md:text-4xl lg:text-5xl font-bold mb-4",
          "Everything Your Fleet Needs. Nothing to Assemble."
        }
        br {}
        p { class: "text-xl max-w-3xl mx-auto",
          "Not a box of parts to wire together — one dashboard where config, firmware, telemetry, and alerts already work with each other. Here's what you get the night you sign up."
        }
      }
      div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-8",
        CapabilityCard {
          icon: rsx! {
            Icon {
              icon: LdDatabase,
              class: "size-10 stroke-primary",
              title: "Database icon",
            }
          },
          accent_bg: "bg-primary-content",
          title: "Config That Converges",
          maturity: None,
          body: "Every device has a desired state and a reported state. Push a config from the dashboard and the device confirms exactly what it applied — over a live WebSocket it lands in about a second, no polling.",
        }
        CapabilityCard {
          icon: rsx! {
            Icon {
              icon: LdHardDriveDownload,
              class: "size-10 stroke-secondary",
              title: "Download icon",
            }
          },
          accent_bg: "bg-secondary-content",
          title: "OTA That Fails Closed",
          maturity: None,
          body: "Upload firmware once, roll it out per device. Every image and every device carries a board tag, and a mismatched assignment is rejected outright — you can't ship an image to hardware it wasn't built for.",
        }
        CapabilityCard {
          icon: rsx! {
            Icon {
              icon: LdLineChart,
              class: "size-10 stroke-primary",
              title: "Line chart icon",
            }
          },
          accent_bg: "bg-primary-content",
          title: "Telemetry Graphs",
          maturity: None,
          body: "Devices report flat key/value telemetry; the dashboard keeps a latest-value snapshot plus queryable history, and you build graphs against any numeric key over any time range you pick.",
        }
        CapabilityCard {
          icon: rsx! {
            Icon {
              icon: LdMapPin,
              class: "size-10 stroke-secondary",
              title: "Map pin icon",
            }
          },
          accent_bg: "bg-secondary-content",
          title: "GPS Asset Tracks",
          maturity: None,
          body: "Report GPS fixes as ordinary telemetry keys and the device's page draws its track — start marker, live position, hover readout. No map-tile service, no extra dependencies.",
        }
        CapabilityCard {
          icon: rsx! {
            Icon {
              icon: LdBellRing,
              class: "size-10 stroke-primary",
              title: "Bell icon",
            }
          },
          accent_bg: "bg-primary-content",
          title: "Alerts to Your Inbox",
          maturity: None,
          body: "Threshold, rate-of-change, and heartbeat conditions on your own telemetry — scoped to one device or a whole flock, delivered by email when they fire and when they clear.",
        }
        CapabilityCard {
          icon: rsx! {
            Icon {
              icon: LdScrollText,
              class: "size-10 stroke-secondary",
              title: "Scroll icon",
            }
          },
          accent_bg: "bg-secondary-content",
          title: "Remote Device Logs",
          maturity: None,
          body: "Structured Zephyr logs ship as dictionary-compressed codes — a fraction of the bytes over a cellular link — into a rolling per-device buffer you can pull from the dashboard.",
        }
      }
    }
  }
}

#[component]
fn CapabilityCard(
  icon: Element,
  accent_bg: &'static str,
  title: &'static str,
  maturity: Option<Maturity>,
  body: &'static str,
) -> Element {
  rsx! {
    div {
      class: "card card-border space-y-6 justify-start bg-base-300 border border-base-content/30 rounded-2xl p-8 card-hover",
      style: "animation-delay: 0.2s;",
      div { class: "card-title space-x-4 flex-wrap",
        div { class: "p-2 rounded-2xl {accent_bg} flex items-center justify-center feature-icon shadow-lg",
          {icon}
        }
        h3 { class: "text-2xl font-bold", "{title}" }
        if let Some(m) = maturity {
          MaturityBadge { maturity: m }
        }
      }
      p { class: "leading-relaxed", "{body}" }
    }
  }
}
