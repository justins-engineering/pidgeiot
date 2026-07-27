// Public, no-signup live demo (launch-eve, no ticket number). Reads real
// telemetry from one real, allowlisted pigeon over dovecote's new
// unauthenticated /demo/pigeons/:id/telemetry* routes (docs/api.md's
// "Public Demo API" section) -- this is the actual platform serving real
// device data, not a mock. Reuses the same TelemetryChart the authenticated
// dashboard uses (components/graph_widget.rs's GraphCard does the
// equivalent live-fetch-then-chart wiring for a signed-in user's own
// pigeon); this page is a leaner, read-only variant of that same idea with
// no add/remove/localStorage-backed graph config, since there's nothing
// for an anonymous visitor to configure.
use crate::Route;
use crate::api::demo;
use crate::components::{ChartSeries, TelemetryChart};
use crate::config::DEMO_PIGEON_ID;
use crate::helpers::sleep_ms;
use capsules::{TelemetryHistoryPoint, TelemetryLatest};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdPlay, LdRadio};

/// Auto-refresh cadence (task brief: "every ~30s while mounted") -- the
/// demo pigeon itself reports every 30s, so this matches its own cadence
/// rather than polling faster than new data could possibly arrive.
const REFRESH_MS: i32 = 30_000;

/// How far back the graphs look. The demo pigeon reports every 30s, so 6
/// hours is a few hundred points per key -- plenty to show a real,
/// visibly-moving line without ever hitting the history route's own 5000-row
/// cap (helpers/telemetry.rs, dovecote).
const HISTORY_HOURS: i64 = 6;

/// (key, chart title, unit suffix for the latest-value stat tile).
/// Matches what the demo pigeon actually reports (CLAUDE.md's task brief:
/// temp_c/humidity_pct/light_lux every 30s, plus soil_moisture_pct and
/// uptime_s). Only the first three get their own chart -- three numeric
/// series compared cleanly fits the page without a key picker; the other
/// two still show in the latest-value strip.
const CHART_KEYS: [(&str, &str, &str); 3] = [
  ("temp_c", "Temperature", "°C"),
  ("humidity_pct", "Humidity", "%"),
  ("light_lux", "Light", "lux"),
];

fn now() -> time::OffsetDateTime {
  time::OffsetDateTime::now_utc()
}

fn series_for_key(key: &str, points: &[TelemetryHistoryPoint]) -> ChartSeries {
  let mut pts: Vec<(i64, f64)> = points
    .iter()
    .filter(|p| p.key == key)
    .filter_map(|p| p.value_num.map(|v| (p.reported_at.unix_timestamp(), v)))
    .collect();
  pts.sort_by_key(|p| p.0);
  ChartSeries {
    key: key.to_string(),
    points: pts,
  }
}

fn latest_value<'a>(latest: &'a [TelemetryLatest], key: &str) -> Option<&'a str> {
  latest
    .iter()
    .find(|l| l.key == key)
    .map(|l| l.value.as_str())
}

/// `uptime_s` reads far more clearly as "12m 34s" than a raw second count.
fn format_uptime(raw: &str) -> String {
  let Ok(secs) = raw.parse::<i64>() else {
    return raw.to_string();
  };
  let h = secs / 3600;
  let m = (secs % 3600) / 60;
  let s = secs % 60;
  if h > 0 {
    format!("{h}h {m}m")
  } else if m > 0 {
    format!("{m}m {s}s")
  } else {
    format!("{s}s")
  }
}

#[component]
pub fn DemoPage() -> Element {
  rsx! {
    section { class: "py-20 md:py-28",
      div { class: "max-w-4xl mx-auto px-4 md:px-8 text-center",
        Icon { icon: LdRadio, class: "w-12 h-12 mx-auto mb-8 stroke-primary", title: "Live" }
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "Live Demo"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-2xl mx-auto text-balance",
          "This is live data from a real PidgeIoT device account -- no signup, no mock data. What's below is the actual platform, reporting in real time."
        }
      }
    }

    if DEMO_PIGEON_ID.is_empty() {
      section { class: "pb-24",
        div { class: "max-w-2xl mx-auto px-4 md:px-8 text-center",
          p { class: "text-base-content/60 italic",
            "The live demo isn't configured in this environment."
          }
        }
      }
    } else {
      DemoContent {}
    }

    section { class: "pb-24 md:pb-32",
      div { class: "max-w-2xl mx-auto px-4 md:px-8 text-center",
        Link {
          class: "btn btn-lg btn-glow font-bold",
          to: Route::RegisterFlow { flow: None },
          Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
          "Start Your Own, Free"
        }
      }
    }
  }
}

#[component]
fn DemoContent() -> Element {
  let mut latest: Signal<Vec<TelemetryLatest>> = use_signal(Vec::new);
  let mut history: Signal<Vec<TelemetryHistoryPoint>> = use_signal(Vec::new);
  let mut loaded_once = use_signal(|| false);

  // Fetches immediately on mount, then every REFRESH_MS thereafter, for as
  // long as this component stays mounted -- Dioxus cancels the future on
  // unmount, same as any other use_future. Never runs during SSG (the
  // synchronous prerender pass doesn't drive async futures to completion --
  // see CLAUDE.md's SSG note on AuthGuard for the same property), so the
  // prerendered page carries the header/empty-state shell, not stale data.
  use_future(move || async move {
    loop {
      let until = now();
      let since = until - time::Duration::hours(HISTORY_HOURS);

      if let Some(l) = demo::get_latest().await {
        latest.set(l);
      }
      if let Some(h) = demo::get_history(since, until).await {
        history.set(h);
      }
      loaded_once.set(true);

      sleep_ms(REFRESH_MS).await;
    }
  });

  let latest_vals = latest();

  rsx! {
    section { class: "pb-16",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        div { class: "stats shadow-sm bg-base-100 border border-base-content/10 w-full grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5",
          for (key , label , unit) in CHART_KEYS {
            div { key: "{key}", class: "stat",
              div { class: "stat-title", "{label}" }
              div { class: "stat-value text-primary text-2xl",
                {
                    latest_value(&latest_vals, key)
                        .map(|v| format!("{v}{unit}"))
                        .unwrap_or_else(|| "--".to_string())
                }
              }
            }
          }
          div { class: "stat",
            div { class: "stat-title", "Soil Moisture" }
            div { class: "stat-value text-primary text-2xl",
              {
                  latest_value(&latest_vals, "soil_moisture_pct")
                      .map(|v| format!("{v}%"))
                      .unwrap_or_else(|| "--".to_string())
              }
            }
          }
          div { class: "stat",
            div { class: "stat-title", "Uptime" }
            div { class: "stat-value text-primary text-2xl",
              {
                  latest_value(&latest_vals, "uptime_s")
                      .map(format_uptime)
                      .unwrap_or_else(|| "--".to_string())
              }
            }
          }
        }

        if !loaded_once() {
          div { class: "flex justify-center py-8",
            span { class: "loading loading-spinner loading-md text-primary" }
          }
        } else if latest_vals.is_empty() {
          p { class: "text-sm text-base-content/50 italic text-center py-8",
            "No telemetry reported yet -- check back shortly."
          }
        }
      }
    }

    if loaded_once() {
      section { class: "pb-16 md:pb-20",
        div { class: "max-w-4xl mx-auto px-4 md:px-8 flex flex-col gap-6",
          for (key , title , _unit) in CHART_KEYS {
            div {
              key: "{key}",
              class: "border border-base-content/10 rounded-box p-4 flex flex-col gap-3 bg-base-100",
              h3 { class: "font-semibold text-lg", "{title}" }
              TelemetryChart { series: vec![series_for_key(key, &history())] }
            }
          }
        }
      }
    }
  }
}
