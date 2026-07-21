use crate::Route;
use crate::components::{Maturity, MaturityBadge};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCodeXml, LdDatabase, LdHardDriveDownload, LdKeyRound, LdLineChart, LdLockKeyhole,
  LdMailWarning, LdNetwork, LdPlay, LdRadio, LdScrollText, LdShieldAlert, LdSquareTerminal, LdWifi,
};

#[component]
pub fn FeaturesPage() -> Element {
  rsx! {
    section { class: "py-24 md:py-32 text-center",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "Everything a Device Fleet Actually Needs"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-3xl mx-auto text-balance",
          "PidgeIoT isn't a pile of primitives you have to wire together. Every piece below ships together, in the same repo, driven by the same shadow model."
        }
      }
    }

    section { class: "pb-16 md:pb-24",
      div { class: "max-w-6xl mx-auto px-4 md:px-8 space-y-20",

        // Configuration
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdDatabase, class: "size-8 stroke-primary", title: "Database icon" }
          },
          eyebrow: "For anyone tired of config drift",
          title: "One Shadow, Always in Sync",
          body: "Every pigeon has a desired state and a reported state, versioned independently. Push a target_config, and the device confirms back exactly what it applied and when — so you always know whether a fleet has actually converged, not just whether you told it to.",
          maturity: None,
        }

        // Security
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdKeyRound, class: "size-8 stroke-primary", title: "Key icon" }
          },
          eyebrow: "For anyone who's had to rotate a leaked API key at 2am",
          title: "Per-Device Keys, Not Shared Secrets",
          body: "Every pigeon gets its own Ed25519 keypair, generated on device creation — only the public key is ever stored, and the private key signs one token before it's discarded. Devices authenticate with a 69-byte binary bearer token, not a JWT. Refreshing a device's token mints a brand-new keypair on the spot, which permanently invalidates the old one; there's no separate revocation list to maintain because overwriting the verification key is the revocation.",
          maturity: None,
        }

        // Telemetry
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdLineChart, class: "size-8 stroke-primary", title: "Line chart icon" }
          },
          eyebrow: "For anyone who's outgrown a spreadsheet of sensor readings",
          title: "Telemetry With Real History, On Your Terms",
          body: "Devices report flat key/value telemetry over HTTPS, CoAP, or the WebSocket channel — captured as a latest-value snapshot and a full queryable history, backed by our own self-hosted GreptimeDB (with an automatic Postgres fallback), so you can build graphs against any key, over any pigeon or an entire flock, with time ranges you pick. Don't want us holding your data at all? Point a pigeon's telemetry endpoint at your own GreptimeDB-compatible line-protocol database and reports go straight there instead.",
          maturity: None,
        }

        // Logging
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdScrollText, class: "size-8 stroke-primary", title: "Scroll icon" }
          },
          eyebrow: "For anyone debugging a device that's three states away",
          title: "Dictionary-Compressed Device Logs",
          body: "The Zephyr device library ships structured logs as dictionary-compressed codes instead of raw strings — a fraction of the bytes over a cellular link. They land in a rolling per-device buffer, and the dashboard's log viewer decodes them back against the firmware's own dictionary, so you get readable log lines without the wire cost.",
          maturity: None,
        }

        // OTA
        FeatureBlock {
          icon: rsx! {
            Icon {
              icon: LdHardDriveDownload,
              class: "size-8 stroke-primary",
              title: "Download icon",
            }
          },
          eyebrow: "For anyone who's bricked a device pushing firmware",
          title: "OTA Updates That Can't Land on the Wrong Hardware",
          body: "Firmware images are content-addressed by SHA-256, stored in R2, and catalogued per flock — assigned to a pigeon through the same shadow model as config. Downloads use Range requests straight into the MCUboot secondary slot, so a device can resume a large image instead of restarting it. Every image and every pigeon carries a board tag, and an assignment is rejected outright unless they match — a fail-closed check against a real incident, not a hypothetical one. Hardware-verified end to end on both the nRF9160 and the nRF9151.",
          maturity: None,
        }

        // Connectivity
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdRadio, class: "size-8 stroke-primary", title: "Radio icon" }
          },
          eyebrow: "For anyone whose hardware can't afford a full HTTPS stack",
          title: "HTTPS or CoAP-over-TLS, Same API",
          body: "No proprietary firmware, no supported-device list. Speak HTTPS if you can, or RFC 8323 CoAP-over-TLS/TCP if your hardware is too constrained for a full HTTPS stack — both hit the same ingestion API, both stay fully encrypted. There's no bare, unencrypted UDP path.",
          maturity: None,
        }

        // Connection state
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdShieldAlert, class: "size-8 stroke-primary", title: "Shield icon" }
          },
          eyebrow: "For anyone who's had a device go dark without noticing",
          title: "At-a-Glance Connection State",
          body: "Every pigeon shows Online, Stale, or Offline right on its card — self-calibrated from that pigeon's own reporting interval, not a single fixed timeout across your whole fleet. No extra device traffic, no new route to wire up; it's derived entirely from data the dashboard already has.",
          maturity: None,
        }

        // Identity
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdLockKeyhole, class: "size-8 stroke-primary", title: "Lock icon" }
          },
          eyebrow: "For anyone who doesn't want to run their own auth stack",
          title: "Self-Hosted Identity, Not a Third-Party Login Box",
          body: "Dashboard accounts run on a self-hosted Ory Kratos instance — your users' credentials never leave infrastructure you control, and account emails (verification, recovery) are sent under our own branding, not a generic template. Device identity is completely separate: a pigeon's Ed25519 keypair proves control of that pigeon, full stop, with no Kratos identity involved at all.",
          maturity: None,
        }

        // WebSocket
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdWifi, class: "size-8 stroke-primary", title: "WiFi icon" }
          },
          eyebrow: "For anyone tired of waiting on the next poll interval",
          title: "A Persistent Channel for Config Pushes That Land Instantly",
          body: "WiFi and mains-powered devices can hold one long-lived WebSocket connection instead of polling — a shadow update reaches the device the moment you push it, and telemetry can ride the same socket. Built on Durable Object hibernation, so an idle connection survives without keeping anything warm. Device-side client is hardware-verified (ESP32-C6, nRF9151); the backend has been proven on staging and hasn't been promoted to production yet.",
          maturity: Some(Maturity::Beta),
        }

        // Remote shell
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdSquareTerminal, class: "size-8 stroke-primary", title: "Terminal icon" }
          },
          eyebrow: "For anyone who's wanted to poke a device without a physical console",
          title: "Owner-Gated Remote Diagnostic Shell",
          body: "Run one diagnostic command on a WebSocket-connected device and get its output back over an ordinary dashboard request — relayed through that device's existing socket, gated to the pigeon's owner, with whatever the device's own command allowlist permits. Ships alongside the WebSocket channel above, so it carries the same staging-verified, not-yet-production status.",
          maturity: Some(Maturity::Beta),
        }

        // Codebase
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdCodeXml, class: "size-8 stroke-primary", title: "Code icon" }
          },
          eyebrow: "For anyone who's read a JS backend's node_modules folder and wept",
          title: "Rust and WebAssembly, End to End",
          body: "The edge router, the dashboard, and the shared wire types are all Rust — the backend compiles to a Cloudflare Worker, the frontend compiles to WebAssembly. One language, one type system, shared structs between frontend and backend so they can't drift apart. AGPL-3.0 licensed and developed in the open.",
          maturity: None,
        }
      }
    }

    // Roadmap
    section { class: "pb-24 md:pb-32",
      div { class: "max-w-6xl mx-auto px-4 md:px-8",
        div { class: "text-center mb-12",
          h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight", "On the Roadmap" }
          p { class: "text-lg text-base-content/70 max-w-2xl mx-auto",
            "Designed, not yet built. Nothing here is reachable in the product today — listed so you know what's coming, not what to expect right now."
          }
        }
        div { class: "grid grid-cols-1 md:grid-cols-3 gap-6",
          RoadmapCard {
            icon: rsx! {
              Icon { icon: LdMailWarning, class: "size-7 stroke-primary", title: "Mail warning icon" }
            },
            title: "Alerts & Triggers",
            body: "User-defined conditions on telemetry, delivered by email.",
          }
          RoadmapCard {
            icon: rsx! {
              Icon { icon: LdDatabase, class: "size-7 stroke-primary", title: "Database icon" }
            },
            title: "Per-Flock Database Isolation",
            body: "Dedicated storage per flock or user, instead of shared multi-tenant tables.",
          }
          RoadmapCard {
            icon: rsx! {
              Icon { icon: LdNetwork, class: "size-7 stroke-primary", title: "Network icon" }
            },
            title: "User-Authored Rule Engine",
            body: "Run your own data-processing logic against incoming telemetry, on Workers for Platforms.",
          }
        }
      }
    }

    section { class: "pb-24 md:pb-32 text-center",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        Link {
          class: "btn btn-primary btn-lg px-10 rounded-full",
          to: Route::RegisterFlow { flow: None },
          Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
          "Start Building"
        }
      }
    }
  }
}

#[component]
fn FeatureBlock(
  icon: Element,
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
  maturity: Option<Maturity>,
) -> Element {
  rsx! {
    div { class: "flex flex-col md:flex-row gap-8 items-start",
      div { class: "shrink-0 p-4 rounded-2xl bg-base-300 border border-base-content/10",
        {icon}
      }
      div {
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2", "{eyebrow}" }
        div { class: "flex items-center gap-3 flex-wrap mb-3",
          h2 { class: "text-2xl md:text-3xl font-bold", "{title}" }
          if let Some(m) = maturity {
            MaturityBadge { maturity: m }
          }
        }
        p { class: "text-lg text-base-content/70 leading-relaxed max-w-3xl", "{body}" }
      }
    }
  }
}

#[component]
fn RoadmapCard(icon: Element, title: &'static str, body: &'static str) -> Element {
  rsx! {
    div { class: "p-6 rounded-2xl bg-base-300/50 border border-base-content/10",
      div { class: "flex items-center gap-3 mb-3",
        div { class: "shrink-0 p-2 rounded-xl bg-base-300", {icon} }
        h3 { class: "text-lg font-bold", "{title}" }
        MaturityBadge { maturity: Maturity::Planned }
      }
      p { class: "text-base-content/70 leading-relaxed", "{body}" }
    }
  }
}
