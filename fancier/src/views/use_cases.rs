use crate::Route;
use crate::components::FeedbackForm;
use dioxus::prelude::*;

#[component]
pub fn UseCasesPage() -> Element {
  let mut feedback = use_context::<FeedbackForm>();

  rsx! {
    section { id: "use-cases-hero", class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Use cases" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "If it reports numbers over a network, it fits."
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-2xl text-base-content/80 text-pretty",
          "Five shapes we designed for, written as example builds. We're in beta — when these become customer stories, we'll say whose."
        }
      }
    }

    section { id: "use-cases-list", class: "px-4 md:px-10 py-14",
      div { class: "max-w-6xl mx-auto flex flex-col",

        UseCase {
          number: "01",
          title: "Vehicle & asset tracking",
          body: "A trailer, a generator, a tool crate. The device reports a fix as ordinary telemetry keys and its page draws the track — start marker, live position, hover readout — with no map-tile contract underneath.",
          body_secondary: "Heartbeat alerts cover the case that actually matters: the asset that stopped reporting.",
          keys: vec!["gps_lat", "gps_lon", "gps_speed_mps", "battery_v"],
          features_used: "GPS tracks · heartbeat alerts · OTA",
          divider: true,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5",
              svg {
                view_box: "0 0 340 170",
                width: "100%",
                height: "170",
                role: "img",
                "aria-label": "Example asset track",
                rect { x: "0", y: "0", width: "340", height: "170", rx: "10", fill: "var(--chart-surface)" }
                g { stroke: "var(--chart-grid)", stroke_width: "1",
                  line { x1: "0", y1: "42", x2: "340", y2: "42" }
                  line { x1: "0", y1: "85", x2: "340", y2: "85" }
                  line { x1: "0", y1: "128", x2: "340", y2: "128" }
                  line { x1: "85", y1: "0", x2: "85", y2: "170" }
                  line { x1: "170", y1: "0", x2: "170", y2: "170" }
                  line { x1: "255", y1: "0", x2: "255", y2: "170" }
                }
                path {
                  d: "M40 140 C 90 132, 96 96, 140 92 S 210 104, 236 70 S 280 40, 306 34",
                  fill: "none",
                  stroke: "var(--chart-series-1)",
                  stroke_width: "3",
                  stroke_linecap: "round",
                }
                circle {
                  cx: "40",
                  cy: "140",
                  r: "6",
                  fill: "var(--chart-surface)",
                  stroke: "var(--chart-series-1)",
                  stroke_width: "3",
                }
                circle { cx: "306", cy: "34", r: "7", fill: "var(--chart-series-1)" }
              }
              div { class: "flex flex-wrap gap-3 mt-3 font-mono text-xs text-base-content/60",
                span { "4h 12m" }
                span { "38.2 km" }
                span { "last fix 3s ago" }
              }
            }
          },
        }

        UseCase {
          number: "02",
          title: "Irrigation & soil",
          body: "Moisture per block, a few bytes a day, a season of battery between visits. Valve positions travel the other way as desired config the node confirms when it next wakes.",
          body_secondary: "Because config converges rather than fires and forgets, a missed window doesn't silently leave a valve in the wrong state.",
          keys: vec!["soil_pct", "valve_open", "rain_mm"],
          features_used: "Config convergence · threshold alerts",
          divider: true,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5 flex flex-col gap-3",
              div { class: "flex items-center justify-between text-sm gap-3",
                span { class: "font-mono truncate", "block-a · soil_pct" }
                span { class: "font-bold", "31%" }
              }
              progress { class: "progress progress-primary", value: "31", max: "100" }
              div { class: "flex items-center justify-between text-sm gap-3",
                span { class: "font-mono truncate", "block-b · soil_pct" }
                span { class: "font-bold", "58%" }
              }
              progress { class: "progress progress-primary", value: "58", max: "100" }
              div { class: "flex items-center justify-between text-sm pt-1 gap-3",
                span { class: "font-mono text-base-content/70 truncate", "valve_open (desired)" }
                span { class: "badge badge-success badge-sm", "true" }
              }
              div { class: "flex items-center justify-between text-sm gap-3",
                span { class: "font-mono text-base-content/70 truncate", "valve_open (reported)" }
                span { class: "badge badge-success badge-sm", "true ✓" }
              }
            }
          },
        }

        UseCase {
          number: "03",
          title: "Industrial machine monitoring",
          body: "Vibration and temperature per motor at a rate that catches a bearing going bad. Rate-of-change alerts fire on the trend, not just the ceiling.",
          body_secondary: "When something does go wrong, pull the device's compressed log buffer without sending anyone to the floor.",
          keys: vec!["rms_mm_s", "temp_c", "run_hours"],
          features_used: "Rate-of-change alerts · remote logs",
          divider: true,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5",
              svg {
                view_box: "0 0 340 150",
                width: "100%",
                height: "150",
                role: "img",
                "aria-label": "Vibration trend with alert threshold",
                rect { x: "0", y: "0", width: "340", height: "150", rx: "10", fill: "var(--chart-surface)" }
                g { stroke: "var(--chart-grid)", stroke_width: "1",
                  line { x1: "0", y1: "40", x2: "340", y2: "40" }
                  line { x1: "0", y1: "80", x2: "340", y2: "80" }
                  line { x1: "0", y1: "120", x2: "340", y2: "120" }
                }
                line {
                  x1: "0",
                  y1: "46",
                  x2: "340",
                  y2: "46",
                  stroke: "var(--chart-series-8)",
                  stroke_width: "2",
                  stroke_dasharray: "6 6",
                }
                path {
                  d: "M12 118 L48 114 L84 116 L120 108 L156 104 L192 96 L228 84 L264 70 L300 54 L328 40",
                  fill: "none",
                  stroke: "var(--chart-series-1)",
                  stroke_width: "3",
                  stroke_linecap: "round",
                }
                circle { cx: "328", cy: "40", r: "6", fill: "var(--chart-series-8)" }
              }
              p { class: "font-mono text-xs text-base-content/60 mt-3",
                "rms_mm_s crossed 4.5 · alert sent 12:04"
              }
            }
          },
        }

        UseCase {
          number: "04",
          title: "Water & utility metering",
          body: "Endpoints sending very little, very reliably. Each meter owns its own object at the edge, so the cost of the ten-thousandth looks like the cost of the tenth.",
          // The design read "the readings never rest with us", which isn't
          // true: the latest value per key is always upserted before any
          // forwarding decision is made. Only history is bypassed.
          body_secondary: "Regulated retention? Point telemetry at your own line-protocol endpoint and the history accumulates there instead of here — we hold only the latest value of each key, which is what the dashboard and alerts read.",
          keys: vec!["litres_total", "flow_lpm", "tamper"],
          features_used: "Bring-your-own database · fleet OTA",
          divider: true,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5 flex flex-col gap-3",
              div { class: "flex items-center gap-3 flex-wrap",
                span { class: "font-bold", "Ingest destination" }
                span { class: "badge badge-ghost font-mono text-[11px] ml-auto", "your endpoint" }
              }
              div { class: "rounded-xl bg-base-100 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/75 overflow-x-auto",
                p { class: "whitespace-nowrap", "POST https://tsdb.yourco.net/write" }
                p { class: "whitespace-nowrap",
                  "meter,id=0417 litres_total=48213 "
                  span { class: "text-success", "202" }
                }
                p { class: "text-base-content/50 whitespace-nowrap", "// platform history store: bypassed" }
              }
              div { class: "flex flex-wrap gap-x-6 gap-y-1 text-sm",
                span {
                  span { class: "font-bold", "1/day" }
                  " report"
                }
                span {
                  span { class: "font-bold", "0" }
                  " gateways"
                }
              }
            }
          },
        }

        UseCase {
          number: "05",
          title: "Smart parking & city infrastructure",
          body: "Bay occupancy across a district, each sensor its own small object served from the edge nearest it. A pilot of five costs almost nothing, and growing it is the same architecture rather than a re-platform.",
          body_secondary: "Public-sector procurement tends to ask who holds the data. The answer is in the licence and the code.",
          keys: vec!["occupied", "since_ts", "rssi"],
          features_used: "Edge-served objects · AGPL self-host",
          divider: false,
          visual: rsx! {
            div { class: "rounded-2xl border border-base-300 bg-base-200 p-5 flex flex-col gap-3",
              div { class: "flex items-center gap-3 flex-wrap",
                span { class: "font-bold", "Zone C · 24 bays" }
                span { class: "badge badge-ghost font-mono text-[11px] ml-auto", "live" }
              }
              div { class: "grid grid-cols-8 gap-2",
                for (i , occupied) in BAYS.iter().enumerate() {
                  span {
                    key: "{i}",
                    class: if *occupied { "rounded bg-primary h-[26px]" } else { "rounded bg-base-300 h-[26px]" },
                  }
                }
              }
              div { class: "flex flex-wrap gap-x-5 gap-y-1 text-sm",
                span {
                  span { class: "font-bold", "14" }
                  " occupied"
                }
                span {
                  span { class: "font-bold", "10" }
                  " free"
                }
                span { class: "text-base-content/60", "updated 2s ago" }
              }
            }
          },
        }
      }
    }

    section { id: "use-cases-cta", class: "px-4 md:px-10 pb-14",
      div {
        class: "max-w-6xl mx-auto rounded-3xl bg-primary px-6 md:px-12 py-14 text-center",
        style: "color:var(--color-primary-content)",
        h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
          "Yours isn't on the list?"
        }
        p { class: "text-lg md:text-xl mt-3",
          "Tell us what it reports and we'll tell you honestly whether we fit."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-7",
          button {
            r#type: "button",
            class: "btn btn-lg font-bold border-0",
            style: "background:var(--color-primary-content);color:var(--color-primary)",
            onclick: move |_| feedback.0.set(true),
            "Talk to us"
          }
          Link {
            class: "btn btn-lg btn-outline font-bold",
            style: "background:transparent;border-color:var(--color-primary-content);color:var(--color-primary-content)",
            to: Route::DemoPage {},
            "Try the live demo"
          }
        }
      }
    }
  }
}

/// Occupancy of the illustrative 24-bay grid, matching the design's layout.
const BAYS: [bool; 24] = [
  true, true, false, true, false, false, true, true, false, true, true, true, false, true, false,
  true, true, false, true, false, true, true, false, false,
];

#[component]
fn UseCase(
  number: &'static str,
  title: &'static str,
  body: &'static str,
  body_secondary: &'static str,
  keys: Vec<&'static str>,
  features_used: &'static str,
  visual: Element,
  divider: bool,
) -> Element {
  rsx! {
    div {
      class: "grid grid-cols-1 lg:grid-cols-12 gap-6 lg:gap-10 items-start py-9",
      class: if divider { "border-b border-base-300" },
      div { class: "lg:col-span-1 font-mono text-sm text-base-content/40 lg:pt-1", "{number}" }
      div { class: "lg:col-span-4 flex flex-col gap-3 min-w-0",
        h2 { class: "text-2xl md:text-3xl font-bold", "{title}" }
        p { class: "leading-relaxed text-base-content/80", "{body}" }
        p { class: "leading-relaxed text-base-content/75", "{body_secondary}" }
      }
      div { class: "lg:col-span-4 w-full min-w-0", {visual} }
      div { class: "lg:col-span-3 flex flex-col gap-3 min-w-0",
        p { class: "text-xs uppercase tracking-widest text-base-content/50", "Telemetry keys" }
        div { class: "flex flex-wrap gap-2",
          for k in keys.iter() {
            span { key: "{k}", class: "badge badge-ghost font-mono text-[11px]", "{k}" }
          }
        }
        p { class: "text-xs uppercase tracking-widest text-base-content/50 mt-2", "Features used" }
        p { class: "text-sm leading-relaxed text-base-content/75", "{features_used}" }
      }
    }
  }
}
