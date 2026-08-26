use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdBellRing, LdKeyRound, LdPlay, LdRadio, LdRefreshCw, LdSend,
};

#[component]
pub fn HowItWorksPage() -> Element {
  rsx! {
    section { id: "how-it-works-hero", class: "py-24 md:py-32 text-center",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "How a Reading Gets From a Device to Your Dashboard"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-3xl mx-auto text-balance",
          "Five steps, end to end. No hidden middleware, no broker to run, and nothing here that isn't already live in the product."
        }
      }
    }

    section { id: "how-it-works-stops", class: "pb-16 md:pb-24",
      div { class: "max-w-5xl mx-auto px-4 md:px-8 space-y-16",

        Stop {
          number: "01",
          icon: rsx! {
            Icon { icon: LdKeyRound, class: "size-8 stroke-primary", title: "Key icon" }
          },
          title: "Provision the device and get its key",
          body: "Create a flock, then create a pigeon inside it. That call mints the device's identity: its own Ed25519 keypair, generated server-side inside the isolated Durable Object that will later verify it. The private key signs exactly one bearer token and is then discarded; only the public key is ever written to storage, and it never leaves the object that checks against it. The token itself is 69 bytes of binary (a version byte, a 4-byte expiry, and a 64-byte signature), not a JWT, and it carries no device id at all: the binding to a specific pigeon comes from which pigeon's stored public key verifies the signature. The response is the only time the token is ever returned, so save it then.",
          code: Some("POST /flock/pigeons\n{\"flock_id\":\"…\",\"name\":\"Coop Sensor 1\",\n \"connector\":{\"Https\":{\"endpoint\":\"\",\"token\":\"\"}}}\n\n201 Created  →  connector.Https.token  (shown once)"),
        }

        Stop {
          number: "02",
          icon: rsx! {
            Icon { icon: LdRadio, class: "size-8 stroke-primary", title: "Radio icon" }
          },
          title: "Pick a transport that suits the hardware",
          body: "The same device API is reachable three ways. Plain HTTPS is the simplest and works anywhere. A device on mains power or WiFi can instead hold one long-lived WebSocket, so config reaches it the instant you push it rather than at the next poll: same credential, same routes, just a persistent channel. And for hardware too constrained to carry a full HTTPS stack, a pigeon can be given a CoAP connector instead: a dedicated terminator speaks both DTLS/UDP and RFC 8323 TLS/TCP, each authenticated by its own per-device pre-shared key, and proxies into the very same ingestion API. There is no unencrypted path on any of the three.",
          code: None,
        }

        Stop {
          number: "03",
          icon: rsx! {
            Icon { icon: LdSend, class: "size-8 stroke-primary", title: "Send icon" }
          },
          title: "The device reports telemetry",
          body: "A report is a flat JSON object of string key/value pairs: no nesting, no schema for us to enforce, and no types to negotiate ahead of time. In production the gateway verifies the bearer token, queues the report and answers 202 immediately, so a device on a slow cellular link isn't holding a socket open waiting on a database write. Values come back out as strings, with a parsed numeric alongside them wherever the value happens to be a number, so numeric series can be plotted without a cast.",
          code: Some("POST /device/pigeons/<id>/telemetry\nAuthorization: Bearer <device_token>\n{\"temp_c\":\"21.5\",\"battery_v\":\"3.9\"}\n\n202 Accepted"),
        }

        Stop {
          number: "04",
          icon: rsx! {
            Icon { icon: LdRefreshCw, class: "size-8 stroke-primary", title: "Refresh icon" }
          },
          title: "Config converges through the shadow",
          body: "Every pigeon holds a desired state and a reported state, versioned independently. You write a target_config from the dashboard; the device applies what it understands and writes back its own current_config with the version it actually reached. That difference is the whole point: you can see whether a fleet has genuinely converged, not merely whether you told it to. On a WebSocket-connected device the update is pushed the moment you save it rather than waiting for the next poll.",
          code: None,
        }

        Stop {
          number: "05",
          icon: rsx! {
            Icon { icon: LdBellRing, class: "size-8 stroke-primary", title: "Bell icon" }
          },
          title: "The platform watches, so you don't have to",
          body: "From there the ordinary operational surface takes over. Alerts email you when a value crosses a threshold, jumps further between reports than it plausibly should, or when a device simply stops reporting; that last one runs on a scheduled sweep, because silence can't be noticed at ingest time. Firmware is content-addressed by SHA-256 and assigned through the same shadow model as config, with a board tag on both image and device that must match before an assignment is allowed. Device logs land in a rolling per-device buffer, dictionary-compressed on the wire and decoded back in the dashboard.",
          code: None,
        }
      }
    }

    section { id: "how-it-works-cta", class: "pb-24 md:pb-32 text-center",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight",
          "You can run all five without owning hardware"
        }
        p { class: "text-lg text-base-content/70 mb-8 leading-relaxed",
          "The device library builds for Zephyr's native_sim target, so the whole path above runs on your own machine in about ten minutes."
        }
        div { class: "flex flex-col sm:flex-row gap-4 justify-center items-center",
          Link {
            class: "btn btn-primary btn-lg px-10 rounded-full",
            to: Route::GettingStartedPage {},
            Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
            "Getting Started"
          }
          Link {
            class: "btn btn-ghost btn-lg px-10 rounded-full",
            to: Route::ApiReferencePage {},
            "Read the API reference"
          }
        }
      }
    }
  }
}

#[component]
fn Stop(
  number: &'static str,
  icon: Element,
  title: &'static str,
  body: &'static str,
  code: Option<&'static str>,
) -> Element {
  rsx! {
    div { class: "flex flex-col md:flex-row gap-8 items-start",
      div { class: "shrink-0 flex flex-col items-center gap-3",
        div { class: "p-4 rounded-2xl bg-base-300 border border-base-content/10", {icon} }
        span { class: "text-sm font-mono font-bold text-base-content/40", "{number}" }
      }
      // w-full is load-bearing: `items-start` sizes a column child to its
      // max-content width, and the code sample's longest line would drag
      // the whole text column past the viewport, where it gets clipped
      // rather than scrolled.
      div { class: "w-full min-w-0 flex-1",
        h2 { class: "text-2xl md:text-3xl font-bold mb-3", "{title}" }
        p { class: "text-lg text-base-content/70 leading-relaxed max-w-3xl", "{body}" }
        if let Some(sample) = code {
          div { class: "mockup-code mt-6 text-sm w-full max-w-full overflow-x-auto",
            pre { class: "px-5 whitespace-pre-wrap break-words", code { "{sample}" } }
          }
        }
      }
    }
  }
}
