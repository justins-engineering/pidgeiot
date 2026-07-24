// Hand-rolled SVG line chart (task #19: "NO new JS/chart dependencies").
// Follows the house dataviz method: fixed categorical hue order (never
// cycled), 2px round-cap/join lines, 8px surface-ringed end markers, a
// legend for 2+ series (a single series gets a direct end-label instead),
// a crosshair + one-tooltip-for-every-series hover layer, and a table-view
// fallback so every value is reachable without hovering.
//
// The canvas is a FIXED pixel size (not a percentage-scaled viewBox) so
// mouse `element_coordinates()` map 1:1 onto SVG user-space units without
// needing a JS/getBoundingClientRect round trip to recover a scale factor;
// the wrapping div scrolls horizontally on narrow viewports instead of
// distorting that mapping.
use dioxus::prelude::*;

const CANVAS_W: f64 = 640.0;
const CANVAS_H: f64 = 220.0;
const MARGIN_LEFT: f64 = 48.0;
const MARGIN_RIGHT: f64 = 12.0;
const MARGIN_TOP: f64 = 12.0;
const MARGIN_BOTTOM: f64 = 28.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
  pub key: String,
  /// (unix seconds, value), ascending by time.
  pub points: Vec<(i64, f64)>,
}

fn series_color_class(index: usize) -> &'static str {
  // Capped at the palette's 8 validated slots — see tailwind.css. A 9th
  // series folds into an "+N more" note rather than generating a new hue.
  const CLASSES: [&str; 8] = [
    "chart-series-1",
    "chart-series-2",
    "chart-series-3",
    "chart-series-4",
    "chart-series-5",
    "chart-series-6",
    "chart-series-7",
    "chart-series-8",
  ];
  CLASSES[index % CLASSES.len()]
}

/// Rounds a raw axis step up to a "nice" 1/2/5 * 10^n step so tick labels
/// read as clean numbers rather than e.g. 173.4.
fn nice_step(raw: f64) -> f64 {
  if raw <= 0.0 || !raw.is_finite() {
    return 1.0;
  }
  let magnitude = 10f64.powf(raw.log10().floor());
  let residual = raw / magnitude;
  let step = if residual > 5.0 {
    10.0
  } else if residual > 2.0 {
    5.0
  } else if residual > 1.0 {
    2.0
  } else {
    1.0
  };
  step * magnitude
}

fn format_value(v: f64) -> String {
  if v.abs() >= 1000.0 {
    format!("{:.0}", v)
  } else if v.fract().abs() < 0.001 {
    format!("{:.0}", v)
  } else {
    format!("{:.2}", v)
  }
}

/// `pub(crate)` (not just module-private) so `components::track_widget`
/// can reuse the exact same "Mon DD HH:MM" formatting for its own hover
/// tooltip instead of a second copy drifting out of sync.
pub(crate) fn format_time(unix: i64) -> String {
  let format = time::macros::format_description!("[month repr:short] [day] [hour]:[minute]");
  time::OffsetDateTime::from_unix_timestamp(unix)
    .ok()
    .and_then(|t| t.format(&format).ok())
    .unwrap_or_else(|| "--".to_string())
}

#[component]
pub fn TelemetryChart(series: Vec<ChartSeries>) -> Element {
  let mut show_table = use_signal(|| false);
  let mut hover_time = use_signal(|| None::<i64>);

  let plottable: Vec<&ChartSeries> = series.iter().filter(|s| !s.points.is_empty()).collect();

  if plottable.is_empty() {
    return rsx! {
      div { class: "text-sm text-base-content/50 italic py-8 text-center",
        "No numeric telemetry points in this range yet."
      }
    };
  }

  let t_min = plottable
    .iter()
    .filter_map(|s| s.points.first().map(|p| p.0))
    .min()
    .unwrap_or(0);
  let t_max = plottable
    .iter()
    .filter_map(|s| s.points.last().map(|p| p.0))
    .max()
    .unwrap_or(t_min + 1);
  let t_span = (t_max - t_min).max(1) as f64;

  let v_min_raw = plottable
    .iter()
    .flat_map(|s| s.points.iter().map(|p| p.1))
    .fold(f64::INFINITY, f64::min);
  let v_max_raw = plottable
    .iter()
    .flat_map(|s| s.points.iter().map(|p| p.1))
    .fold(f64::NEG_INFINITY, f64::max);
  let pad = ((v_max_raw - v_min_raw).abs() * 0.1).max(1.0);
  let v_min = v_min_raw - pad;
  let v_max = v_max_raw + pad;
  let v_span = (v_max - v_min).max(f64::EPSILON);

  let plot_w = CANVAS_W - MARGIN_LEFT - MARGIN_RIGHT;
  let plot_h = CANVAS_H - MARGIN_TOP - MARGIN_BOTTOM;

  let x_of = move |t: i64| MARGIN_LEFT + ((t - t_min) as f64 / t_span) * plot_w;
  let y_of = move |v: f64| MARGIN_TOP + (1.0 - (v - v_min) / v_span) * plot_h;

  let y_step = nice_step((v_max_raw - v_min_raw) / 3.0);
  let first_tick = (v_min / y_step).ceil() * y_step;
  let mut y_ticks = Vec::new();
  let mut tick = first_tick;
  while tick <= v_max {
    y_ticks.push(tick);
    tick += y_step;
  }

  let show_legend = plottable.len() >= 2;
  let overflow_count = series.len().saturating_sub(8);

  let hover_series = series.clone();
  let hover_x = hover_time().map(x_of);

  rsx! {
    div { class: "w-full flex flex-col gap-2",
      div { class: "flex items-center justify-end",
        button {
          class: "btn btn-ghost btn-xs text-base-content/60",
          r#type: "button",
          onclick: move |_| show_table.toggle(),
          if show_table() { "View as chart" } else { "View as table" }
        }
      }

      if show_table() {
        div { class: "overflow-x-auto",
          table { class: "table table-sm",
            thead {
              tr {
                th { "Time" }
                for s in plottable.iter() {
                  th { "{s.key}" }
                }
              }
            }
            tbody {
              for (t , _) in plottable[0].points.iter() {
                tr {
                  td { class: "font-mono text-xs", "{format_time(*t)}" }
                  for s in plottable.iter() {
                    td { class: "font-mono text-xs",
                      {
                          s.points
                              .iter()
                              .find(|p| p.0 == *t)
                              .map(|p| format_value(p.1))
                              .unwrap_or_else(|| "--".to_string())
                      }
                    }
                  }
                }
              }
            }
          }
        }
      } else {
        div { class: "relative w-full overflow-x-auto",
          svg {
            width: "{CANVAS_W}",
            height: "{CANVAS_H}",
            view_box: "0 0 {CANVAS_W} {CANVAS_H}",
            class: "min-w-[{CANVAS_W}px]",

            // Gridlines + y ticks
            for v in y_ticks.iter() {
              g { key: "{v}",
                line {
                  x1: "{MARGIN_LEFT}",
                  x2: "{CANVAS_W - MARGIN_RIGHT}",
                  y1: "{y_of(*v)}",
                  y2: "{y_of(*v)}",
                  stroke: "var(--chart-grid)",
                  stroke_width: "1",
                }
                text {
                  x: "{MARGIN_LEFT - 6.0}",
                  y: "{y_of(*v) + 3.0}",
                  text_anchor: "end",
                  font_size: "9",
                  fill: "var(--chart-ink-secondary)",
                  "{format_value(*v)}"
                }
              }
            }

            // Baseline
            line {
              x1: "{MARGIN_LEFT}",
              x2: "{CANVAS_W - MARGIN_RIGHT}",
              y1: "{CANVAS_H - MARGIN_BOTTOM}",
              y2: "{CANVAS_H - MARGIN_BOTTOM}",
              stroke: "var(--chart-axis)",
              stroke_width: "1",
            }

            // X ticks: start and end timestamps only, to stay uncluttered.
            text {
              x: "{MARGIN_LEFT}",
              y: "{CANVAS_H - 8.0}",
              text_anchor: "start",
              font_size: "9",
                  fill: "var(--chart-ink-secondary)",
              "{format_time(t_min)}"
            }
            text {
              x: "{CANVAS_W - MARGIN_RIGHT}",
              y: "{CANVAS_H - 8.0}",
              text_anchor: "end",
              font_size: "9",
                  fill: "var(--chart-ink-secondary)",
              "{format_time(t_max)}"
            }

            // Series lines + end markers
            for (i , s) in plottable.iter().enumerate() {
              g { key: "{s.key}", class: "{series_color_class(i)}",
                polyline {
                  points: {
                      s.points
                          .iter()
                          .map(|(t, v)| format!("{},{}", x_of(*t), y_of(*v)))
                          .collect::<Vec<_>>()
                          .join(" ")
                  },
                  fill: "none",
                  stroke: "currentColor",
                  stroke_width: "2",
                  stroke_linecap: "round",
                  stroke_linejoin: "round",
                }
                if let Some((t, v)) = s.points.last() {
                  circle {
                    cx: "{x_of(*t)}",
                    cy: "{y_of(*v)}",
                    r: "6",
                    fill: "var(--chart-surface)",
                  }
                  circle {
                    cx: "{x_of(*t)}",
                    cy: "{y_of(*v)}",
                    r: "4",
                    fill: "currentColor",
                  }
                  if !show_legend {
                    text {
                      x: "{x_of(*t) + 8.0}",
                      y: "{y_of(*v) + 3.0}",
                      font_size: "10",
                      fill: "var(--chart-ink-secondary)",
                      "{s.key}: {format_value(*v)}"
                    }
                  }
                }
              }
            }

            // Crosshair
            if let Some(x) = hover_x {
              line {
                x1: "{x}",
                x2: "{x}",
                y1: "{MARGIN_TOP}",
                y2: "{CANVAS_H - MARGIN_BOTTOM}",
                stroke: "var(--chart-axis)",
                stroke_width: "1",
              }
            }

            // Hover hit area — sized to the plot area, in the same
            // viewBox units as everything above (see module doc comment).
            rect {
              x: "{MARGIN_LEFT}",
              y: "{MARGIN_TOP}",
              width: "{plot_w}",
              height: "{plot_h}",
              fill: "transparent",
              onmousemove: move |evt: Event<MouseData>| {
                  let point = evt.data().element_coordinates();
                  let rel_x = point.x.clamp(0.0, plot_w);
                  let t = t_min + ((rel_x / plot_w) * t_span) as i64;
                  let nearest = hover_series
                      .iter()
                      .flat_map(|s| s.points.iter().map(|p| p.0))
                      .min_by_key(|candidate| (candidate - t).abs());
                  hover_time.set(nearest);
              },
              onmouseleave: move |_| hover_time.set(None),
            }
          }

          // Tooltip: one row per series at the hovered time, values leading
          // (Strong), series name secondary — per interaction.md.
          if let Some(t) = hover_time() {
            div {
              class: "absolute top-2 pointer-events-none bg-base-100 border border-base-content/10 rounded-box shadow-lg px-3 py-2 text-xs",
              style: "left: {(x_of(t) + 12.0).min(CANVAS_W - 160.0)}px;",
              div { class: "text-base-content/60 font-mono mb-1", "{format_time(t)}" }
              for (i , s) in plottable.iter().enumerate() {
                {
                    let nearest = s.points.iter().min_by_key(|p| (p.0 - t).abs());
                    rsx! {
                      div { key: "{s.key}", class: "flex items-center gap-2",
                        span { class: "inline-block w-3 h-0.5 {series_color_class(i)} bg-current" }
                        span { class: "font-semibold text-base-content",
                          {nearest.map(|p| format_value(p.1)).unwrap_or_else(|| "--".to_string())}
                        }
                        span { class: "text-base-content/60", "{s.key}" }
                      }
                    }
                }
              }
            }
          }
        }
      }

      if show_legend {
        div { class: "flex flex-wrap gap-x-4 gap-y-1",
          for (i , s) in plottable.iter().enumerate() {
            div { key: "{s.key}", class: "flex items-center gap-1.5 text-xs text-base-content/70",
              span { class: "inline-block w-3 h-0.5 {series_color_class(i)} bg-current" }
              "{s.key}"
            }
          }
        }
      }

      if overflow_count > 0 {
        div { class: "text-[11px] text-base-content/50",
          "+{overflow_count} more key(s) not shown — pick fewer to compare them."
        }
      }
    }
  }
}
