// GPS/asset-tracking track view for the pigeon detail page -- the
// dashboard-side counterpart to a device sample (built in parallel) that
// reports GPS fixes as ordinary numeric telemetry keys (see
// `helpers::gps_track`'s module doc comment for the exact key set).
//
// Self-contained SVG (same "no new JS/chart dependencies" constraint as
// `components::telemetry_chart` -- see that module's own doc comment),
// not a tile-based map: no Leaflet/OSM, no external network fetch, works
// fully offline and themes natively off the app's own DaisyUI tokens
// instead of a tile provider's fixed light/dark styling. A later upgrade
// to real map tiles would mean swapping this SVG polyline/marker layer
// for a Leaflet (or MapLibre GL) instance underneath the same
// time-range/hover chrome -- explicitly out of scope for this pass.
//
// All the bounding-box/projection/nearest-point math lives in
// `helpers::gps_track` as pure, unit-tested functions; this component is
// just the SVG + DaisyUI presentation over them, the same split
// `telemetry_chart.rs`/`graph_widget.rs` already use.
use crate::api::telemetry;
use crate::components::graph_widget::{GraphDef, TimeRange};
use crate::components::telemetry_chart::format_time;
use crate::helpers::gps_track::{self, Bounds, GpsFix, TrackProjector, current_position_line};
use capsules::TelemetryLatest;
use dioxus::prelude::*;
use time::OffsetDateTime;

const CANVAS_W: f64 = 480.0;
const CANVAS_H: f64 = 380.0;
const MARGIN: f64 = 16.0;
// Extra whitespace around the track's own bounding box so a fix at the
// very edge isn't drawn flush against the plot border -- same idea as
// `TelemetryChart`'s y-axis padding, just fractional here since the two
// axes share one scale (`TrackProjector`).
const PAD_FRAC: f64 = 0.15;

fn now() -> OffsetDateTime {
  OffsetDateTime::now_utc()
}

#[component]
pub fn TrackWidget(
  pigeon_id: String,
  /// The pigeon's latest telemetry snapshot -- already fetched by
  /// `PigeonView` for the connection badge (see `views::pigeon`'s own
  /// doc comment on that reuse). Used here only for the "current
  /// position" summary line; the track itself comes from a separate
  /// history fetch below since a single "latest" snapshot can't show
  /// movement over time.
  latest: Vec<TelemetryLatest>,
  /// Fired by the "+ Speed graph"/"+ Altitude graph" quick-add buttons --
  /// the caller (`PigeonView`) forwards this into `PigeonGraphs`' own
  /// `quick_add` inbox so the new graph shows up in the Telemetry section
  /// without a page reload. This widget doesn't touch `localStorage`
  /// itself; see `PigeonGraphs`' doc comment on why the write has to
  /// happen there instead.
  on_quick_add: EventHandler<GraphDef>,
) -> Element {
  let mut range = use_signal(|| TimeRange::Last24h);
  let mut fixes: Signal<Option<Vec<GpsFix>>> = use_signal(|| None);
  let mut hover_index: Signal<Option<usize>> = use_signal(|| None);

  {
    let pigeon_id = pigeon_id.clone();
    use_resource(move || {
      let pigeon_id = pigeon_id.clone();
      async move {
        let until = now();
        let since = until - time::Duration::seconds(range().seconds());
        let result = telemetry::get_history(&pigeon_id, since, until).await;
        fixes.set(result.map(|points| gps_track::gps_fixes_from_history(&points)));
        hover_index.set(None);
      }
    });
  }

  let position_line = current_position_line(&latest);
  let plot_w = CANVAS_W - 2.0 * MARGIN;
  let plot_h = CANVAS_H - 2.0 * MARGIN;

  let body = match fixes.read().as_ref() {
    None => rsx! {
      div { class: "flex items-center justify-center py-16",
        span { class: "loading loading-spinner loading-md text-primary" }
      }
    },
    Some(fx) if fx.is_empty() => rsx! {
      div { class: "text-sm text-base-content/50 italic py-16 text-center",
        "No GPS fixes reported in this range yet."
      }
    },
    Some(fx) => {
      let bounds = Bounds::of(fx).expect("checked non-empty above");
      let lon_scale = gps_track::lon_scale_for_lat(bounds.mean_lat());
      let stationary = fx.len() == 1 || gps_track::is_stationary(&bounds, lon_scale);

      if stationary {
        let last = fx.last().expect("checked non-empty above");
        let single = fx.len() == 1;
        rsx! {
          div { class: "flex flex-col items-center gap-3 py-4",
            svg {
              width: "{CANVAS_W}",
              height: "{CANVAS_H * 0.55}",
              view_box: "0 0 {CANVAS_W} {CANVAS_H * 0.55}",
              class: "min-w-[{CANVAS_W}px]",
              rect {
                x: "0",
                y: "0",
                width: "{CANVAS_W}",
                height: "{CANVAS_H * 0.55}",
                fill: "var(--chart-surface)",
                rx: "8",
              }
              circle {
                cx: "{CANVAS_W / 2.0}",
                cy: "{CANVAS_H * 0.275}",
                r: "16",
                fill: "var(--color-primary)",
                opacity: "0.25",
                class: "animate-pulse",
              }
              circle {
                cx: "{CANVAS_W / 2.0}",
                cy: "{CANVAS_H * 0.275}",
                r: "6",
                fill: "var(--color-primary)",
              }
            }
            p { class: "text-xs text-base-content/60 text-center px-4",
              if single {
                "Single GPS fix in this range — "
              } else {
                "Stationary in this range — no meaningful movement detected. "
              }
              "{gps_track::format_coord(last.lat, last.lon)}"
            }
          }
        }
      } else {
        let projector = TrackProjector::new(&bounds, plot_w, plot_h, PAD_FRAC);
        let projected: Vec<(f64, f64)> = fx.iter().map(|f| projector.project(f.lat, f.lon)).collect();
        let path_points = projected
          .iter()
          .map(|(x, y)| format!("{x},{y}"))
          .collect::<Vec<_>>()
          .join(" ");
        let (start_x, start_y) = projected[0];
        let (end_x, end_y) = *projected.last().expect("checked non-empty above");

        let hover_i = hover_index();
        let hovered = hover_i.and_then(|i| fx.get(i).zip(projected.get(i).copied()));
        let tooltip = hovered.map(|(fix, (hx, hy))| {
          let time_label = format_time(fix.reported_at);
          let coord_label = gps_track::format_coord(fix.lat, fix.lon);
          let speed_label = fix.speed_mps.map(|s| format!("{s:.1} m/s"));
          (hx, hy, time_label, coord_label, speed_label)
        });

        rsx! {
          div { class: "relative w-full overflow-x-auto",
            svg {
              width: "{CANVAS_W}",
              height: "{CANVAS_H}",
              view_box: "0 0 {CANVAS_W} {CANVAS_H}",
              class: "min-w-[{CANVAS_W}px]",
              rect {
                x: "0",
                y: "0",
                width: "{CANVAS_W}",
                height: "{CANVAS_H}",
                fill: "var(--chart-surface)",
                rx: "8",
              }
              g { transform: "translate({MARGIN}, {MARGIN})",
                polyline {
                  points: "{path_points}",
                  fill: "none",
                  stroke: "var(--color-primary)",
                  stroke_width: "2",
                  stroke_linecap: "round",
                  stroke_linejoin: "round",
                }
                // Start marker: hollow -- the track's beginning.
                circle {
                  cx: "{start_x}",
                  cy: "{start_y}",
                  r: "6",
                  fill: "var(--chart-surface)",
                  stroke: "var(--color-primary)",
                  stroke_width: "2",
                }
                // Latest marker: solid + pulsing, same "live" convention
                // as `ConnectionBadge`'s online status dot.
                circle {
                  cx: "{end_x}",
                  cy: "{end_y}",
                  r: "8",
                  fill: "var(--color-primary)",
                  opacity: "0.35",
                  class: "animate-pulse",
                }
                circle {
                  cx: "{end_x}",
                  cy: "{end_y}",
                  r: "4",
                  fill: "var(--color-primary)",
                }
                if let Some((hx, hy, ..)) = tooltip.as_ref() {
                  circle {
                    cx: "{hx}",
                    cy: "{hy}",
                    r: "6",
                    fill: "none",
                    stroke: "var(--chart-ink-primary)",
                    stroke_width: "1.5",
                  }
                }
                rect {
                  x: "0",
                  y: "0",
                  width: "{plot_w}",
                  height: "{plot_h}",
                  fill: "transparent",
                  onmousemove: move |evt: Event<MouseData>| {
                      let point = evt.data().element_coordinates();
                      hover_index.set(gps_track::nearest_point_index(&projected, (point.x, point.y)));
                  },
                  onmouseleave: move |_| hover_index.set(None),
                }
              }
            }

            if let Some((hx, _hy, time_label, coord_label, speed_label)) = tooltip {
              div {
                class: "absolute top-2 pointer-events-none bg-base-100 border border-base-content/10 rounded-box shadow-lg px-3 py-2 text-xs",
                style: "left: {(hx + MARGIN + 12.0).min(CANVAS_W - 190.0)}px;",
                div { class: "text-base-content/60 font-mono mb-1", "{time_label}" }
                div { class: "font-semibold text-base-content", "{coord_label}" }
                if let Some(speed) = speed_label {
                  div { class: "text-base-content/70", "{speed}" }
                }
              }
            }
          }
        }
      }
    }
  };

  rsx! {
    div { class: "w-full flex flex-col gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4 flex-wrap",
        div {
          h2 { class: "text-3xl font-bold", "GPS Track" }
          if let Some(line) = position_line.as_ref() {
            p { class: "text-sm text-base-content/70 font-mono mt-1", "{line}" }
          }
        }
        select {
          class: "select select-bordered select-sm",
          value: "{range().label()}",
          onchange: move |evt: Event<FormData>| {
              if let Some(r) = TimeRange::from_label(&evt.value()) {
                  range.set(r);
              }
          },
          for r in TimeRange::ALL {
            option { value: "{r.label()}", selected: r == range(), "{r.label()}" }
          }
        }
      }

      div { class: "flex flex-wrap gap-2 md:px-4",
        button {
          class: "btn btn-outline btn-xs",
          r#type: "button",
          onclick: move |_| {
              on_quick_add
                  .call(GraphDef {
                      id: uuid::Uuid::now_v7().to_string(),
                      title: "Speed".to_string(),
                      keys: vec![gps_track::KEY_SPEED_MPS.to_string()],
                      range: range(),
                  });
          },
          "+ Speed graph"
        }
        button {
          class: "btn btn-outline btn-xs",
          r#type: "button",
          onclick: move |_| {
              on_quick_add
                  .call(GraphDef {
                      id: uuid::Uuid::now_v7().to_string(),
                      title: "Altitude".to_string(),
                      keys: vec![gps_track::KEY_ALT_M.to_string()],
                      range: range(),
                  });
          },
          "+ Altitude graph"
        }
      }

      {body}
    }
  }
}
