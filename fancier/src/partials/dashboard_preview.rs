// Marketing depiction of the signed-in dashboard. It deliberately mirrors
// what the real dashboard shows today rather than the design's mockup: the
// design drew a tabbed flock shell (Overview/Devices/Firmware/Alerts/Logs)
// that doesn't exist, and a firmware rollout aggregate that can't exist
// until devices report the version they're actually running. Claiming
// "this is the whole dashboard" only works if the picture is the product.
use dioxus::prelude::*;

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
pub fn DashboardPreview() -> Element {
  rsx! {
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
