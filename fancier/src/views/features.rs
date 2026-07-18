use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCodeXml, LdDatabase, LdHardDriveDownload, LdKeyRound, LdLineChart, LdLockKeyhole, LdPlay,
  LdRadio, LdScrollText,
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

    section { class: "pb-24 md:pb-32",
      div { class: "max-w-6xl mx-auto px-4 md:px-8 space-y-20",

        // Configuration
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdDatabase, class: "size-8 stroke-primary", title: "Database icon" }
          },
          eyebrow: "For anyone tired of config drift",
          title: "One Shadow, Always in Sync",
          body: "Every pigeon has a desired state and a reported state, versioned independently. Push a target_config, and the device confirms back exactly what it applied and when — so you always know whether a fleet has actually converged, not just whether you told it to.",
          badge: None,
        }

        // Security
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdKeyRound, class: "size-8 stroke-primary", title: "Key icon" }
          },
          eyebrow: "For anyone who's had to rotate a leaked API key at 2am",
          title: "Per-Device Keys, Not Shared Secrets",
          body: "Every pigeon gets its own Ed25519 keypair, generated on device creation — only the public key is ever stored, and the private key signs one token before it's discarded. Devices authenticate with a 69-byte binary bearer token, not a JWT. Refreshing a device's token mints a brand-new keypair on the spot, which permanently invalidates the old one; there's no separate revocation list to maintain because overwriting the verification key is the revocation.",
          badge: None,
        }

        // Telemetry
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdLineChart, class: "size-8 stroke-primary", title: "Line chart icon" }
          },
          eyebrow: "For anyone who's outgrown a spreadsheet of sensor readings",
          title: "Telemetry With Real History, On Your Terms",
          body: "Devices report flat key/value telemetry that's queryable with range history, not just a last-known-value snapshot — build graphs against any key, over any pigeon or an entire flock, with time ranges you pick. Don't want us holding your data at all? Point a pigeon's telemetry endpoint at your own GreptimeDB-compatible line-protocol database and reports go straight there instead.",
          badge: Some("Queue-buffered ingest: rolling out to production"),
        }

        // Logging
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdScrollText, class: "size-8 stroke-primary", title: "Scroll icon" }
          },
          eyebrow: "For anyone debugging a device that's three states away",
          title: "Dictionary-Compressed Device Logs",
          body: "The Zephyr device library ships structured logs as dictionary-compressed codes instead of raw strings — a fraction of the bytes over a cellular link. The dashboard's log viewer decodes them back against the firmware's own dictionary, so you get readable log lines without the wire cost.",
          badge: Some("Rolling out"),
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
          title: "OTA Updates That Can Undo Themselves",
          body: "Firmware images are content-addressed by SHA-256 and stored in R2, assigned to a pigeon through the same shadow model as config. Downloads use Range requests straight into the MCUboot secondary slot, so a device can resume a large image instead of restarting it. The device only confirms a new image as good after it boots successfully — an image that never confirms gets automatically reverted on the next boot.",
          badge: Some("Rolling out — staging-verified, hardware end-to-end pending"),
        }

        // Connectivity
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdRadio, class: "size-8 stroke-primary", title: "Radio icon" }
          },
          eyebrow: "For anyone whose hardware can't afford a full HTTPS stack",
          title: "HTTPS or CoAP-over-TLS, Same API",
          body: "No proprietary firmware, no supported-device list. Speak HTTPS if you can, or RFC 8323 CoAP-over-TLS/TCP if your hardware is too constrained for a full HTTPS stack — both hit the same ingestion API, both stay fully encrypted. There's no bare, unencrypted UDP path.",
          badge: None,
        }

        // Identity
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdLockKeyhole, class: "size-8 stroke-primary", title: "Lock icon" }
          },
          eyebrow: "For anyone who doesn't want to run their own auth stack",
          title: "Self-Hosted Identity, Not a Third-Party Login Box",
          body: "Dashboard accounts run on a self-hosted Ory Kratos instance — your users' credentials never leave infrastructure you control. Device identity is completely separate: a pigeon's Ed25519 keypair proves control of that pigeon, full stop, with no Kratos identity involved at all.",
          badge: None,
        }

        // Codebase
        FeatureBlock {
          icon: rsx! {
            Icon { icon: LdCodeXml, class: "size-8 stroke-primary", title: "Code icon" }
          },
          eyebrow: "For anyone who's read a JS backend's node_modules folder and wept",
          title: "Rust and WebAssembly, End to End",
          body: "The edge router, the dashboard, and the shared wire types are all Rust — the backend compiles to a Cloudflare Worker, the frontend compiles to WebAssembly. One language, one type system, shared structs between frontend and backend so they can't drift apart. AGPL-3.0 licensed and developed in the open.",
          badge: None,
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
  badge: Option<&'static str>,
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
          if let Some(b) = badge {
            span { class: "badge badge-sm badge-primary", "{b}" }
          }
        }
        p { class: "text-lg text-base-content/70 leading-relaxed max-w-3xl", "{body}" }
      }
    }
  }
}
