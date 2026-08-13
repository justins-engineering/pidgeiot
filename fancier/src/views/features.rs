use crate::Route;
use dioxus::prelude::*;

#[component]
pub fn FeaturesPage() -> Element {
  rsx! {
    section { class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Features" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "Six things every fleet needs. All of them on day one."
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-2xl text-base-content/80 text-pretty",
          "Not a box of primitives to wire together — identity, config, firmware, telemetry, alerts and logs designed against each other in one codebase."
        }
      }
    }

    section { class: "px-4 md:px-10 py-14",
      div { class: "max-w-6xl mx-auto flex flex-col gap-12",

        // 01 — Identity. The design claimed the keypair is minted on the
        // device and the private half never leaves it; mint_device_credential
        // generates it server-side in the pigeon's own Durable Object, signs
        // one token and drops the private key, so the copy says that instead.
        FeatureRow {
          eyebrow: "01 — Identity",
          title: "A key per device, minted where its state lives",
          body: "Each pigeon gets its own Ed25519 keypair, generated inside the isolated object that will later verify it. The private half signs one token and is discarded on the spot — only the public key is ever stored. That token is 69 bytes: version, expiry, signature. Authentication costs almost nothing on a metered link.",
          body_secondary: rsx! {
            "Refreshing a token overwrites the old public key, which means rotation "
            span { class: "italic", "is" }
            " revocation. There's no fleet-wide secret to leak and no revocation list to sync."
          },
          reverse: false,
          visual: rsx! {
            div { class: "mockup-code text-sm w-full max-w-full overflow-x-auto",
              pre { class: "px-5",
                code { class: "text-base-content/50", "// the entire device auth path" }
              }
              pre { class: "px-5",
                code { class: "text-primary", "pub fn " }
                code { "verify_device_token(token: &str, key_b64: &str) -> bool {{" }
              }
              pre { class: "px-5",
                code { "    let Ok(raw) = URL_SAFE_NO_PAD.decode(token) else {{ .. }};" }
              }
              pre { class: "px-5", code { "    if raw.len() != 69 {{ return false }}" } }
              pre { class: "px-5",
                code { "    let (payload, sig) = raw.split_at(5);  // ver + expiry" }
              }
              pre { class: "px-5", code { "    key.verify(payload, &sig).is_ok()" } }
              pre { class: "px-5", code { "}}" } }
            }
          },
        }

        FeatureRow {
          eyebrow: "02 — Config",
          title: "Set what you want. See what landed.",
          body: "Every device carries a desired state and a reported state. You push the first; the device confirms the second with exactly what it applied — so \"configured\" is a fact, not an assumption.",
          body_secondary: rsx! {
            "Over a live socket it lands in about a second. Devices that sleep pick it up on their next check-in and confirm then."
          },
          reverse: true,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5 md:p-7 flex flex-col gap-4",
              div { class: "flex items-center gap-3 flex-wrap",
                span { class: "font-mono text-sm text-base-content/60", "pigeon-0417" }
                span { class: "badge badge-success badge-sm ml-auto", "converged" }
              }
              div { class: "grid grid-cols-1 sm:grid-cols-2 gap-4",
                div { class: "rounded-xl bg-base-100 border border-base-300 p-4 flex flex-col gap-2 min-w-0",
                  p { class: "text-xs uppercase tracking-widest text-base-content/50", "Desired" }
                  p { class: "font-mono text-sm truncate", "report_interval: 60s" }
                  p { class: "font-mono text-sm truncate", "gps_enabled: true" }
                }
                div { class: "rounded-xl bg-base-100 border border-base-300 p-4 flex flex-col gap-2 min-w-0",
                  p { class: "text-xs uppercase tracking-widest text-base-content/50", "Reported" }
                  p { class: "font-mono text-sm text-success truncate", "report_interval: 60s ✓" }
                  p { class: "font-mono text-sm text-success truncate", "gps_enabled: true ✓" }
                }
              }
              p { class: "text-xs font-mono text-base-content/50",
                "applied 1.1s after push · live over WebSocket, no polling"
              }
            }
          },
        }

        // 03 — Firmware. The design paired this with a fleet rollout
        // aggregate ("39 of 42 on latest", "2 downloading"); nothing reports
        // a device's running firmware version back to us, so there is no
        // such number to render and the widget is left out.
        FeatureRow {
          eyebrow: "03 — Firmware",
          title: "OTA that refuses to brick the wrong board",
          body: "Upload an image once and roll it out per device or per flock. Images and devices both carry a board tag, and a mismatched assignment is rejected outright.",
          body_secondary: rsx! {
            "Images live content-addressed by their own SHA-256, and devices resume interrupted downloads with Range requests instead of starting the whole file again."
          },
          reverse: false,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5 md:p-7 flex flex-col gap-4",
              div { class: "flex items-center gap-3 flex-wrap",
                p { class: "font-bold text-lg", "firmware · v1.4.2" }
                span { class: "badge badge-ghost font-mono text-[11px] ml-auto", "board: nrf9160" }
              }
              div { class: "rounded-xl bg-base-100 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/75 overflow-x-auto",
                p { class: "whitespace-nowrap", "sha256  9f2c…a41b   612 KiB" }
                p { class: "whitespace-nowrap",
                  "assign → pigeon-0417  "
                  span { class: "text-success", "board matches ✓" }
                }
                p { class: "whitespace-nowrap",
                  "assign → pigeon-0902  "
                  span { class: "text-error", "board mismatch — refused" }
                }
              }
              p { class: "text-xs font-mono text-base-content/50",
                "resumable · Range requests straight into the secondary slot"
              }
            }
          },
        }

        div { class: "grid grid-cols-1 md:grid-cols-3 gap-6",

          FeatureCard {
            eyebrow: "04 — Telemetry",
            title: "Graphs and tracks, no setup",
            body: "Devices report flat key/value pairs. You get a latest-value snapshot, queryable history, a graph against any numeric key, and a GPS track when the keys are a fix.",
            visual: rsx! {
              svg {
                view_box: "0 0 320 90",
                width: "100%",
                height: "90",
                role: "img",
                "aria-label": "Example telemetry chart",
                rect { x: "0", y: "0", width: "320", height: "90", rx: "8", fill: "var(--chart-surface)" }
                g { stroke: "var(--chart-grid)", stroke_width: "1",
                  line { x1: "0", y1: "30", x2: "320", y2: "30" }
                  line { x1: "0", y1: "60", x2: "320", y2: "60" }
                }
                path {
                  d: "M10 70 L48 60 L86 66 L124 44 L162 50 L200 30 L238 38 L276 22 L310 28",
                  fill: "none",
                  stroke: "var(--chart-series-1)",
                  stroke_width: "2.5",
                  stroke_linecap: "round",
                }
                path {
                  d: "M10 78 L48 74 L86 80 L124 70 L162 74 L200 62 L238 68 L276 56 L310 60",
                  fill: "none",
                  stroke: "var(--chart-series-2)",
                  stroke_width: "2.5",
                  stroke_linecap: "round",
                }
              }
            },
          }

          FeatureCard {
            eyebrow: "05 — Alerts",
            title: "Email when it matters",
            body: "Thresholds, rate-of-change and heartbeats on your own keys, scoped to one device or a whole flock — mailed when they fire and again when they clear.",
            visual: rsx! {
              div { class: "rounded-xl bg-base-100 border border-base-300 p-4 flex flex-col gap-2 font-mono text-xs text-base-content/75 overflow-x-auto",
                p { class: "whitespace-nowrap",
                  span { class: "text-error", "FIRING" }
                  " temp_c > 30 for 5m · pigeon-0440"
                }
                p { class: "whitespace-nowrap",
                  span { class: "text-success", "CLEARED" }
                  " battery_v < 3.4 · pigeon-0421"
                }
                p { class: "whitespace-nowrap",
                  span { class: "text-warning", "HEARTBEAT" }
                  " no report in 2h · pigeon-0440"
                }
              }
            },
          }

          FeatureCard {
            eyebrow: "06 — Logs",
            title: "Remote logs that fit the link",
            body: "Structured Zephyr logs ship as dictionary-compressed codes — a fraction of the bytes over cellular — into a rolling per-device buffer you pull on demand.",
            visual: rsx! {
              div { class: "rounded-xl bg-base-100 border border-base-300 p-4 flex flex-col gap-2 font-mono text-xs text-base-content/75 overflow-x-auto",
                p { class: "whitespace-nowrap", "12:04:18 <inf> modem: attach ok, rsrp -91" }
                p { class: "whitespace-nowrap", "12:04:19 <dbg> pigeon: token ok (69 B)" }
                p { class: "whitespace-nowrap", "12:04:21 <wrn> sensor: retry 1/3" }
              }
            },
          }
        }
      }
    }

    section { class: "px-4 md:px-10 py-12 bg-base-200 border-y border-base-300",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 lg:grid-cols-12 gap-6 lg:gap-10 items-center",
        div { class: "lg:col-span-7 flex flex-col gap-3",
          h2 { class: "text-2xl md:text-3xl font-bold", "What isn't here yet" }
          p { class: "text-lg leading-relaxed text-base-content/80",
            "A user-authored rule engine — your own logic running against incoming telemetry at the edge — is designed and not built. We'd rather list it here than imply it ships today. Everything else on this page is running now."
          }
        }
        div { class: "lg:col-span-5 flex flex-wrap gap-3",
          span { class: "badge badge-ghost font-mono", "rule engine · planned" }
          span { class: "badge badge-ghost font-mono", "beta · pre-revenue" }
        }
      }
    }

    section { class: "px-4 md:px-10 py-14",
      div { class: "max-w-6xl mx-auto flex flex-col md:flex-row md:items-center gap-8",
        div {
          h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight mb-2",
            "Easier to see than to read about."
          }
          // The public demo is one real allowlisted device, not a flock.
          p { class: "text-lg md:text-xl text-base-content/70",
            "A real device is reporting into the demo page right now — live, no signup."
          }
        }
        div { class: "md:ml-auto flex flex-col sm:flex-row gap-3 shrink-0",
          Link { class: "btn btn-primary btn-lg font-bold", to: Route::DemoPage {},
            "Try the live demo"
          }
          Link { class: "btn btn-outline btn-lg font-bold", to: Route::DocumentationPage {},
            "Read the docs"
          }
        }
      }
    }
  }
}

#[component]
fn FeatureRow(
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
  body_secondary: Element,
  visual: Element,
  reverse: bool,
) -> Element {
  rsx! {
    div { class: "grid grid-cols-1 lg:grid-cols-12 gap-8 lg:gap-10 items-center",
      div {
        class: "lg:col-span-5 flex flex-col gap-3 min-w-0",
        class: if reverse { "lg:order-2" },
        span { class: "font-mono text-xs uppercase tracking-widest text-primary", "{eyebrow}" }
        h2 { class: "text-2xl md:text-3xl font-bold", "{title}" }
        p { class: "text-lg leading-relaxed text-base-content/80", "{body}" }
        p { class: "text-base leading-relaxed text-base-content/75", {body_secondary} }
      }
      div {
        class: "lg:col-span-7 w-full min-w-0",
        class: if reverse { "lg:order-1" },
        {visual}
      }
    }
  }
}

#[component]
fn FeatureCard(
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
  visual: Element,
) -> Element {
  rsx! {
    div { class: "rounded-2xl border border-base-300 bg-base-200 p-5 md:p-7 flex flex-col gap-3 min-w-0",
      span { class: "font-mono text-xs uppercase tracking-widest text-primary", "{eyebrow}" }
      h3 { class: "text-xl md:text-2xl font-bold", "{title}" }
      p { class: "leading-relaxed text-base-content/80", "{body}" }
      {visual}
    }
  }
}
