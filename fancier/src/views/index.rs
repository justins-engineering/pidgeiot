// Section order follows the chosen homepage design: hook, then the product
// itself, then how it gets there, then who it's for, then the licence. The
// `why` block is the quieter investors/incubators section and must stay
// below all of the user-facing sections, just above the closing CTA.
//
// These seven sections lived in their own files under partials/ until each
// turned out to be used exactly once, here. Inlined so the whole page, and
// every section id on it, is visible by reading one file.

use crate::Route;
use crate::components::{Maturity, MaturityBadge};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdKeyRound, LdMail, LdScrollText, LdServer};

// Marketing depiction of the signed-in dashboard. It deliberately mirrors
// what the real dashboard shows today rather than the design's mockup: the
// design drew a tabbed flock shell (Overview/Devices/Firmware/Alerts/Logs)
// that doesn't exist, and a firmware rollout aggregate that can't exist
// until devices report the version they're actually running. Claiming
// "this is the whole dashboard" only works if the picture is the product.

/// One device row in the illustrative list. `state` names the same four
/// connection states `capsules::connection_state::classify` produces --
/// the design drew only three and had no treatment for a device that has
/// never reported.
struct DeviceRow {
  id: &'static str,
  state: &'static str,
  age: &'static str,
}

const DEVICES: [DeviceRow; 5] = [
  DeviceRow {
    id: "pigeon-0417",
    state: "online",
    age: "3s",
  },
  DeviceRow {
    id: "pigeon-0418",
    state: "online",
    age: "11s",
  },
  DeviceRow {
    id: "pigeon-0421",
    state: "stale",
    age: "6m",
  },
  DeviceRow {
    id: "pigeon-0433",
    state: "unknown",
    age: "never",
  },
  DeviceRow {
    id: "pigeon-0440",
    state: "offline",
    age: "4h",
  },
];

fn status_class(state: &str) -> &'static str {
  match state {
    "online" => "status status-success",
    "stale" => "status status-warning",
    "offline" => "status status-error",
    // Never-reported deliberately avoids DaisyUI's neutral, which is
    // invisible against this theme's base in both light and dark.
    _ => "status bg-base-content/40",
  }
}

#[component]
fn Kpi(label: &'static str, value: &'static str, tone: &'static str) -> Element {
  rsx! {
    div {
      p { class: "text-xs uppercase tracking-widest text-base-content/50", "{label}" }
      p { class: "text-4xl font-extrabold mt-1 {tone}", "{value}" }
    }
  }
}

#[component]
fn Legend(color: &'static str, label: &'static str) -> Element {
  rsx! {
    span { class: "flex items-center gap-1.5",
      span { class: "size-2 rounded-full", style: "background:{color}" }
      "{label}"
    }
  }
}

#[component]
fn Stop(number: &'static str, title: &'static str, body: &'static str, mock: Element) -> Element {
  rsx! {
    div { class: "rounded-2xl border border-base-300 bg-base-100 p-6 md:p-7 flex flex-col gap-4 min-w-0",
      div { class: "flex items-center gap-3",
        span {
          class: "size-9 rounded-full bg-primary font-bold flex items-center justify-center shrink-0",
          style: "color:var(--color-primary-content)",
          "{number}"
        }
        h3 { class: "text-xl md:text-2xl font-bold", "{title}" }
      }
      p { class: "leading-relaxed text-base-content/80", "{body}" }
      {mock}
    }
  }
}

struct Card {
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
}

const CARDS: [Card; 5] = [
  Card {
    eyebrow: "Fleet",
    title: "Vehicle & asset tracking",
    body: "Tracks drawn from plain GPS telemetry. Alerts when an asset moves — or stops reporting.",
  },
  Card {
    eyebrow: "Farm",
    title: "Irrigation & soil",
    body: "Moisture per block, valves as config, a season of battery between visits.",
  },
  Card {
    eyebrow: "Factory",
    title: "Machine monitoring",
    body: "Vibration trends, rate-of-change alarms, remote logs when something's off.",
  },
  Card {
    eyebrow: "Utility",
    title: "Water metering",
    body: "Small payloads, sent reliably, with history in your own database if you prefer.",
  },
  // The design promised "same shape at 5 units or 50,000". The
  // architecture holds, but the dashboard has no paginated device list
  // yet, so the claim is made about the edge model rather than a fleet
  // size we can't render.
  Card {
    eyebrow: "City",
    title: "Smart parking",
    body: "Bay occupancy served from the edge nearest each sensor — one object per device, however many there are.",
  },
];

/// The investors/incubators section — deliberately placed after every
/// user-facing section and deliberately quieter than them (no gradient
/// boxes, no animation): the page sells to individual builders first, and
/// this section only states architecture and position that the rest of the
/// page has already demonstrated. Hard rule: no invented traction,
/// customers, or numbers here, ever. The roadmap item carries a `Planned`
/// badge for the same reason.

#[component]
pub fn Index() -> Element {
  rsx! {
    section { id: "home-hero", class: "pt-20 pb-14 text-center",
      div { class: "max-w-5xl mx-auto",
        h1 { class: "text-5xl md:text-7xl font-extrabold tracking-tight max-w-4xl mx-auto text-pretty",
          "Carrier pigeons for your sensors."
          br {}
          span { class: "text-primary", "Considerably faster." }
        }
        p { class: "mt-7 text-xl md:text-2xl leading-relaxed max-w-2xl mx-auto text-base-content/80 text-pretty",
          "An open-source platform that provisions your devices, keeps their config and firmware current, and brings their readings home."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-9",
          Link { class: "btn btn-primary btn-lg font-bold", to: Route::DemoPage {},
            "Try the live demo"
          }
          a {
            class: "btn btn-outline btn-lg font-bold",
            href: "https://github.com/justins-engineering",
            target: "_blank",
            rel: "noopener noreferrer",
            "Read the source"
          }
        }
        p { class: "mt-5 text-sm text-base-content/60 font-mono",
          "no signup · no hardware · AGPL-3.0"
        }
      }
    }

    section { id: "home-dashboard", class: "pt-14 pb-16",
      div { class: "max-w-6xl mx-auto",
        div { class: "text-center mb-9",
          h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
            "This is the whole dashboard"
          }
          p { class: "text-lg text-base-content/70 mt-2",
            "No modules to buy, no widgets to assemble. Every device looks like this the moment it checks in."
          }
        }

        div { class: "rounded-2xl border border-base-300 bg-base-200 p-3",
          div { class: "flex items-center gap-2 px-2 py-2",
            span { class: "size-2.5 rounded-full bg-error" }
            span { class: "size-2.5 rounded-full bg-warning" }
            span { class: "size-2.5 rounded-full bg-success" }
            span { class: "ml-3 text-xs font-mono text-base-content/50 truncate",
              "pidgeiot.com / flock: west-fleet"
            }
          }

          div { class: "rounded-xl bg-base-100 border border-base-300",
            // Three KPIs, not the design's four: "On latest firmware"
            // needs a per-device running version nothing reports back.
            div { class: "grid grid-cols-1 sm:grid-cols-3 gap-6 px-4 md:px-6 py-6 border-b border-base-300",
              Kpi { label: "Devices", value: "42", tone: "" }
              Kpi { label: "Reporting", value: "38", tone: "text-success" }
              Kpi { label: "Open alerts", value: "1", tone: "text-error" }
            }

            div { class: "grid grid-cols-1 lg:grid-cols-3 gap-6 px-4 md:px-6 py-6",
              div { class: "lg:col-span-2 flex flex-col gap-3 min-w-0",
                div { class: "flex items-center gap-3 flex-wrap",
                  h3 { class: "font-bold text-lg", "Telemetry" }
                  span { class: "ml-auto flex flex-wrap gap-3 text-xs font-mono text-base-content/60",
                    Legend { color: "var(--chart-series-1)", label: "temp_c" }
                    Legend { color: "var(--chart-series-2)", label: "battery_v" }
                    Legend { color: "var(--chart-series-3)", label: "soil_pct" }
                  }
                }
                svg {
                  view_box: "0 0 640 200",
                  width: "100%",
                  height: "200",
                  role: "img",
                  "aria-label": "Telemetry over 24 hours",
                  rect { x: "0", y: "0", width: "640", height: "200", rx: "10", fill: "var(--chart-surface)" }
                  g { stroke: "var(--chart-grid)", stroke_width: "1",
                    line { x1: "40", y1: "30", x2: "632", y2: "30" }
                    line { x1: "40", y1: "72", x2: "632", y2: "72" }
                    line { x1: "40", y1: "114", x2: "632", y2: "114" }
                    line { x1: "40", y1: "156", x2: "632", y2: "156" }
                  }
                  g {
                    fill: "var(--chart-ink-secondary)",
                    font_size: "10",
                    font_family: "ui-monospace,monospace",
                    text { x: "8", y: "34", "30" }
                    text { x: "8", y: "76", "20" }
                    text { x: "8", y: "118", "10" }
                    text { x: "8", y: "160", "0" }
                    text { x: "44", y: "188", "00:00" }
                    text { x: "190", y: "188", "06:00" }
                    text { x: "336", y: "188", "12:00" }
                    text { x: "482", y: "188", "18:00" }
                    text { x: "596", y: "188", "now" }
                  }
                  line { x1: "40", y1: "170", x2: "632", y2: "170", stroke: "var(--chart-axis)", stroke_width: "1" }
                  path {
                    d: "M44 140 L100 132 L156 136 L212 112 L268 118 L324 84 L380 92 L436 62 L492 72 L548 46 L604 52 L628 44",
                    fill: "none",
                    stroke: "var(--chart-series-1)",
                    stroke_width: "2.5",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                  }
                  path {
                    d: "M44 156 L100 152 L156 158 L212 146 L268 150 L324 138 L380 144 L436 130 L492 136 L548 124 L604 128 L628 122",
                    fill: "none",
                    stroke: "var(--chart-series-2)",
                    stroke_width: "2.5",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                  }
                  path {
                    d: "M44 106 L100 114 L156 100 L212 108 L268 96 L324 104 L380 90 L436 98 L492 86 L548 94 L604 82 L628 88",
                    fill: "none",
                    stroke: "var(--chart-series-3)",
                    stroke_width: "2.5",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                  }
                }
              }

              div { class: "flex flex-col gap-3 min-w-0",
                h3 { class: "font-bold text-lg", "Devices" }
                div { class: "flex flex-col",
                  for (i , d) in DEVICES.iter().enumerate() {
                    div {
                      key: "{d.id}",
                      class: "flex items-center gap-3 py-2.5",
                      class: if i + 1 < DEVICES.len() { "border-b border-base-300" },
                      span { class: status_class(d.state) }
                      span { class: "font-mono text-sm grow truncate", "{d.id}" }
                      span { class: "text-xs text-base-content/50 shrink-0", "{d.age}" }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }

    section { id: "home-route", class: "py-16",
      div { class: "max-w-6xl mx-auto",
        h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight text-center",
          "The whole trip, three stops"
        }
        div { class: "grid grid-cols-1 md:grid-cols-3 gap-6 mt-10",

          // The design had the device mint its own key, via a `pidge`
          // CLI. Neither is real: the keypair is minted server-side when
          // the pigeon is registered, and there is no CLI to install.
          Stop {
            number: "1",
            title: "Your device",
            body: "Flash the Zephyr library. Registering the device mints its keypair and hands back a 69-byte token — the private half signs that token and is discarded, so only the public key is ever stored. From there it speaks CoAP over DTLS or plain HTTPS, whatever the modem can afford.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/70 overflow-x-auto",
                p { class: "whitespace-nowrap", "dashboard → Register Pigeon" }
                p { class: "whitespace-nowrap text-success", "✓ keypair minted, public key stored" }
                p { class: "whitespace-nowrap text-success", "✓ token issued (69 B) — shown once" }
              }
            },
          }

          Stop {
            number: "2",
            title: "The edge",
            body: "Each device owns a small object on Cloudflare's network — its shadow, its permissions, its credentials. Nothing to provision, nothing to patch, close to wherever it wakes up.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 flex flex-col gap-2",
                div { class: "flex items-center justify-between text-sm gap-3",
                  span { class: "text-base-content/60", "desired" }
                  span { class: "font-mono", "interval: 60s" }
                }
                div { class: "flex items-center justify-between text-sm gap-3",
                  span { class: "text-base-content/60", "reported" }
                  span { class: "font-mono text-success", "interval: 60s ✓" }
                }
                progress { class: "progress progress-primary", value: "100", max: "100" }
              }
            },
          }

          // The design's sample used a /v1 prefix we don't have and a flat
          // float map; the real route returns one row per key with string
          // values and the timestamp they were reported at.
          Stop {
            number: "3",
            title: "You",
            body: "The dashboard above: graphs, GPS tracks, firmware rollouts, remote logs and alerts by email. Or bypass it — the API the dashboard uses is the API you get.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/70 overflow-x-auto",
                p { class: "whitespace-nowrap", "GET /pigeons/0417/telemetry" }
                p { class: "whitespace-nowrap",
                  span { class: "text-success", "200" }
                  " · [{{\"key\":\"temp_c\",\"value\":\"21.4\","
                }
                p { class: "whitespace-nowrap", "        \"reported_at\":\"…\"}}, …]" }
              }
            },
          }
        }
      }
    }

    section { id: "home-use-cases", class: "py-16",
      div { class: "max-w-6xl mx-auto",
        h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
          "Where it earns its keep"
        }
        p { class: "text-lg text-base-content/70 mt-2 mb-9",
          "Examples, not case studies — we're in beta and we're not going to pretend otherwise."
        }
        div { class: "grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4",
          for c in CARDS.iter() {
            div {
              key: "{c.title}",
              class: "rounded-2xl bg-base-200 border border-base-300 p-6 flex flex-col gap-3 min-w-0",
              span { class: "text-xs font-mono uppercase tracking-widest text-primary", "{c.eyebrow}" }
              h3 { class: "text-lg font-bold leading-snug", "{c.title}" }
              p { class: "text-sm leading-relaxed text-base-content/75", "{c.body}" }
            }
          }
        }
        div { class: "mt-8",
          Link { class: "link link-primary font-semibold", to: Route::UseCasesPage {},
            "See what each one uses →"
          }
        }
      }
    }

    section { id: "home-open-source", class: "py-16",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 md:grid-cols-3 gap-8",

        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Open, all of it" }
          p { class: "leading-relaxed text-base-content/80",
            "AGPL-3.0 across the edge router, the dashboard and the device library. No open core with the useful parts behind a sales call."
          }
          a {
            class: "link link-primary font-semibold text-sm",
            href: "https://github.com/justins-engineering",
            target: "_blank",
            rel: "noopener noreferrer",
            "Browse the repos →"
          }
        }

        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Secure by shape" }
          p { class: "leading-relaxed text-base-content/80",
            "Per-device Ed25519 keys, encrypted transports only, and a token so small it costs nothing to send. One compromised device stays one compromised device."
          }
          Link { class: "link link-primary font-semibold text-sm", to: Route::HowItWorksPage {},
            "Security model →"
          }
        }

        // The design said telemetry sent to your own endpoint is never
        // stored. The latest value per key is always kept — it's what the
        // dashboard renders and what alerts evaluate against — so the
        // claim is scoped to the history, which is the part that's true.
        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Private by default" }
          p { class: "leading-relaxed text-base-content/80",
            "Send telemetry to your own endpoint and the history accumulates there, not here — we keep only the latest value per key. Dashboard identity is self-hosted, so your credentials don't visit a third party."
          }
          Link { class: "link link-primary font-semibold text-sm", to: Route::HowItWorksPage {},
            "How data flows →"
          }
        }
      }
    }

    section { id: "platform", class: "front-page",
      div { class: "max-w-4xl mx-auto",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2 text-center",
          "The long view"
        }
        h2 { class: "text-3xl md:text-4xl font-bold mb-6 text-center", "Why This Platform" }
        p { class: "text-lg leading-relaxed mb-12 text-center text-pretty",
          "Open-source IoT today makes builders choose: assemble a pile of primitives yourself, or pay enterprise prices for the pre-assembled version. PidgeIoT's bet is that one coherent, AGPL-licensed product — identity, config, firmware, telemetry, and alerts designed together in a single codebase — wins the individual developers that the incumbents price out or wear down. Those developers become the small fleets, and the small fleets become the large ones."
        }
        div { class: "space-y-8",
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdServer,
                class: "size-7 stroke-primary",
                title: "Server icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Serverless economics, edge-native by default" }
              p { class: "leading-relaxed text-base-content/80",
                "The backend runs on Cloudflare Workers and Durable Objects — each device owns its own SQLite-backed object at the edge. No idle servers to pay for, no capacity planning: a fleet of five costs almost nothing to serve, and the same architecture serves a fleet of thousands without a re-platform."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdKeyRound,
                class: "size-7 stroke-primary",
                title: "Key icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Cryptographic identity per device" }
              p { class: "leading-relaxed text-base-content/80",
                "Every device authenticates with its own Ed25519 keypair and a 69-byte binary token — no shared secrets, no JWT overhead, and refreshing a token is revocation, because it overwrites the only key the old one could verify against. Dashboard identity is self-hosted Ory Kratos: user credentials never leave infrastructure we control."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdScrollText,
                class: "size-7 stroke-primary",
                title: "Scroll icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Rust and WebAssembly, end to end" }
              p { class: "leading-relaxed text-base-content/80",
                "The edge router, this dashboard, and the wire types between them are one Rust workspace — the backend compiles to a Worker, the frontend to WebAssembly, and shared structs mean the two cannot drift apart. The protocol itself is the product surface: everything the dashboard does rides the same documented API a device or a script can use."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-b border-base-content/10 py-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdServer,
                class: "size-7 stroke-secondary",
                title: "Server icon",
              }
            }
            div {
              div { class: "flex items-center gap-3 flex-wrap mb-2",
                h3 { class: "text-xl font-bold", "Next: a user-authored rule engine" }
                MaturityBadge { maturity: Maturity::Planned }
              }
              p { class: "leading-relaxed text-base-content/80",
                "Designed, not yet built: user-written logic running against incoming telemetry at the edge, on Cloudflare Workers for Platforms — the step from device management to a programmable platform."
              }
            }
          }
        }
        div { class: "mt-12 text-center",
          p { class: "leading-relaxed text-base-content/70 max-w-2xl mx-auto mb-6",
            "PidgeIoT is in beta and pre-revenue, and this page says so. There are no customer logos here because we haven't earned them yet — the public repos, the commit history, and the running product are the evidence. If you're evaluating us, read the code."
          }
          div { class: "flex flex-col sm:flex-row justify-center gap-4",
            Link {
              class: "btn btn-outline rounded-full font-bold",
              to: Route::Architecture {},
              "Read the Architecture"
            }
            a {
              class: "btn btn-outline rounded-full font-bold",
              href: "mailto:code@jes.contact",
              Icon { icon: LdMail, class: "mr-2", title: "Email" }
              "Talk to Us"
            }
          }
        }
      }
    }

    section { id: "home-cta", class: "my-16",
      div {
        class: "max-w-6xl mx-auto rounded-3xl bg-primary px-6 md:px-12 py-16 text-center",
        style: "color:var(--color-primary-content)",
        h2 { class: "text-4xl md:text-5xl font-extrabold tracking-tight", "Send up your first bird" }
        // The design said "the demo flock is already flying"; the public
        // demo is a single allowlisted device.
        p { class: "text-lg md:text-xl mt-4",
          "A real device is already reporting. Ten minutes, no hardware, no card."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-8",
          Link {
            class: "btn btn-lg font-bold border-0",
            style: "background:var(--color-primary-content);color:var(--color-primary)",
            to: Route::DemoPage {},
            "Try the live demo"
          }
          Link {
            class: "btn btn-lg btn-outline font-bold",
            style: "background:transparent;border-color:var(--color-primary-content);color:var(--color-primary-content)",
            to: Route::DocumentationPage {},
            "Read the docs"
          }
        }
      }
    }
  }
}
