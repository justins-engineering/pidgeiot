// Hand-rolled SVG chart -- no new JS/chart dependency. Follows the
// house dataviz method: fixed categorical hue order (never
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
//
// `ChartKind` is where most of the judgment in this file lives. Telemetry
// history is an irregular, unaggregated event log -- devices report when
// they report, and nothing on the server buckets or resamples it. Each
// shape asserts something different about the space *between* two samples,
// and only some of those assertions are ones this data can support, so the
// kinds that would otherwise lie (area, bar) carry the transform that
// makes them true rather than being drawn naively over raw samples.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

const CANVAS_W: f64 = 640.0;
const CANVAS_H: f64 = 220.0;
const MARGIN_LEFT: f64 = 48.0;
const MARGIN_RIGHT: f64 = 12.0;
const MARGIN_TOP: f64 = 12.0;
const MARGIN_BOTTOM: f64 = 28.0;

/// House mark spec: a gap in the surface color, never a stroke drawn round
/// the mark, is what separates two touching fills.
const SURFACE_GAP: f64 = 2.0;
/// Bars are capped rather than filling their slot -- the leftover is air.
const MAX_BAR_WIDTH: f64 = 24.0;
const MAX_BUCKETS: usize = 32;

/// The palette's eight validated slots. A ninth series folds into a "+N
/// more" note rather than generating a hue.
const MAX_SERIES: usize = 8;

/// Scatter draws every series onto one plane, so any two of them can end up
/// side by side -- the palette has to clear the validator's `--pairs all`
/// run, not just the adjacent-pairs one every other kind here is judged by.
/// This repo's slot order clears it for its first four slots (worst
/// all-pairs CVD ΔE 13.0 light / 6.9 dark); the dark figure sits in the 6-8
/// band that is legal only alongside secondary encoding, which is what
/// `marker_path`'s per-slot shapes are. A fifth scatter series is a
/// series-count cap, not a palette problem to solve -- see the dataviz
/// skill's `color-formula.md` on why re-ordering cannot fix an all-pairs
/// floor.
const MAX_SCATTER_SERIES: usize = 4;

/// One SVG node per sample is where this chart stops being interactive, and
/// a single-key graph can carry the server's whole per-response cap. Past
/// this the marks are strided and the chart says so.
const MAX_SCATTER_MARKS: usize = 1200;

/// How a series is drawn. Persisted per saved graph (see
/// `components::graph_widget::GraphDef`), so the variant names are wire
/// format -- renaming one silently resets every saved graph carrying it
/// back to the `Default`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartKind {
  #[default]
  Line,
  Step,
  Area,
  Bar,
  Scatter,
}

impl ChartKind {
  pub const ALL: [ChartKind; 5] = [
    ChartKind::Line,
    ChartKind::Step,
    ChartKind::Area,
    ChartKind::Bar,
    ChartKind::Scatter,
  ];

  pub fn label(self) -> &'static str {
    match self {
      ChartKind::Line => "Line",
      ChartKind::Step => "Step",
      ChartKind::Area => "Area",
      ChartKind::Bar => "Bar",
      ChartKind::Scatter => "Scatter",
    }
  }

  pub fn from_label(label: &str) -> Option<ChartKind> {
    ChartKind::ALL.into_iter().find(|k| k.label() == label)
  }

  /// What this shape claims about the data, in the user's words. Shown
  /// under the picker: choosing between these is a judgment about the
  /// signal being plotted, and nobody can make it from five nouns.
  pub fn describes(self) -> &'static str {
    match self {
      ChartKind::Line => {
        "Draws straight between samples, which reads as the value having moved smoothly between reports."
      }
      ChartKind::Step => {
        "Holds each value until the next report. Honest for anything sampled or discrete — it never claims a reading nobody took."
      }
      ChartKind::Area => {
        "A line with the space down to zero filled, so the axis always includes zero."
      }
      ChartKind::Bar => {
        "The mean of each time bucket, drawn from zero. A bucket with no reports is left empty, not drawn as zero."
      }
      ChartKind::Scatter => {
        "One mark per sample and nothing in between. Shows the real reporting cadence, including its gaps."
      }
    }
  }

  /// Area and bar both measure from the baseline, so the baseline has to be
  /// a real zero rather than wherever the data happens to start -- a filled
  /// region or a bar length only means anything against zero.
  fn needs_zero_baseline(self) -> bool {
    matches!(self, ChartKind::Area | ChartKind::Bar)
  }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
  pub key: String,
  /// (unix seconds, value), ascending by time.
  pub points: Vec<(i64, f64)>,
}

/// A horizontal line at a fixed value -- an alert threshold, today. Dashed
/// on purpose: gridlines and axes here are solid hairlines precisely so
/// that a dashed rule is never mistaken for chrome.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartReference {
  pub value: f64,
  pub label: String,
  /// The alert is currently firing. Carried by a status colour *and* by the
  /// label saying so -- a status never means anything by hue alone.
  pub firing: bool,
}

/// How far outside the plotted data a reference may sit and still be worth
/// putting on the axis. A threshold set near the operating range is the
/// point of drawing it; one set orders of magnitude away would flatten the
/// series into a straight line at the edge, so past this it is reported in
/// words instead of silently wrecking the chart.
const REFERENCE_RANGE_TOLERANCE: f64 = 2.0;

/// Splits references into the ones the axis can accommodate and the ones
/// too far outside the data to draw without destroying it.
fn partition_references(
  references: &[ChartReference],
  v_min: f64,
  v_max: f64,
) -> (Vec<ChartReference>, Vec<ChartReference>) {
  let span = (v_max - v_min).abs().max(f64::EPSILON);
  let slack = span * REFERENCE_RANGE_TOLERANCE;
  references
    .iter()
    .cloned()
    .partition(|r| r.value >= v_min - slack && r.value <= v_max + slack)
}

fn series_color_class(index: usize) -> &'static str {
  // Capped at the palette's 8 validated slots — see tailwind.css. A 9th
  // series folds into an "+N more" note rather than generating a new hue.
  const CLASSES: [&str; MAX_SERIES] = [
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

/// `pub(crate)` so `components::graph_widget` can phrase a pigeon's
/// reporting cadence the same way a bar chart phrases its bucket width,
/// rather than growing a second rounding rule that disagrees with this one.
pub(crate) fn format_duration(secs: i64) -> String {
  if secs < 60 {
    format!("{secs}s")
  } else if secs < 3600 {
    format!("{} min", secs / 60)
  } else if secs < 86_400 {
    format!("{} h", secs / 3600)
  } else {
    format!("{} d", secs / 86_400)
  }
}

/// Geometry for a step line: hold each value until the next sample's
/// timestamp, then jump. Applied at draw time only -- the tooltip and the
/// table keep reading the real samples, so the step corners never become
/// values anyone can read off as data.
fn step_geometry(points: &[(i64, f64)]) -> Vec<(i64, f64)> {
  let mut out: Vec<(i64, f64)> = Vec::with_capacity(points.len() * 2);
  for (t, v) in points {
    if let Some((_, prev)) = out.last().copied() {
      out.push((*t, prev));
    }
    out.push((*t, *v));
  }
  out
}

/// Fixed-width time buckets holding the mean of whatever samples landed in
/// each. Bar is the one kind whose marks do not map onto samples one for
/// one: a bar's *width* reads as a span of time, so drawing one bar per
/// irregularly-spaced sample would make that width mean nothing. Buckets
/// with no samples produce no bar at all -- a gap in reporting is not a
/// measurement of zero.
fn bucket_series(
  series: &[ChartSeries],
  t_min: i64,
  t_max: i64,
  buckets: usize,
) -> Vec<ChartSeries> {
  let span = (t_max - t_min).max(1) as f64;
  let width = span / buckets as f64;

  series
    .iter()
    .map(|s| {
      let mut sums = vec![(0.0f64, 0usize); buckets];
      for (t, v) in &s.points {
        let offset = (*t - t_min) as f64 / width;
        let index = (offset.floor() as usize).min(buckets - 1);
        sums[index].0 += *v;
        sums[index].1 += 1;
      }
      ChartSeries {
        key: s.key.clone(),
        points: sums
          .into_iter()
          .enumerate()
          .filter(|(_, (_, count))| *count > 0)
          .map(|(i, (sum, count))| {
            let centre = t_min as f64 + width * (i as f64 + 0.5);
            (centre as i64, sum / count as f64)
          })
          .collect(),
      }
    })
    .collect()
}

/// Wide enough for every bar in the bucket to exist as a mark rather than a
/// sliver: more series means fewer, wider buckets, not a finer comb.
fn bucket_count(plot_w: f64, series_count: usize, sample_count: usize) -> usize {
  let min_slot = 6.0 * series_count.max(1) as f64 + 8.0;
  let by_width = (plot_w / min_slot).floor().max(1.0) as usize;
  by_width.min(MAX_BUCKETS).min(sample_count.max(1)).max(1)
}

/// A bar with its data-end rounded and its baseline end square. Mirrors the
/// rounded corners when the cap sits below the baseline, which is what a
/// negative reading (`rssi_dbm`, a below-zero temperature) draws as.
fn bar_path(x: f64, width: f64, y_base: f64, y_cap: f64) -> String {
  let height = (y_cap - y_base).abs();
  let r = 4.0_f64.min(width / 2.0).min(height);
  let x2 = x + width;
  if y_cap <= y_base {
    let shoulder = y_cap + r;
    let (left, right) = (x + r, x2 - r);
    format!(
      "M {x} {y_base} L {x} {shoulder} A {r} {r} 0 0 1 {left} {y_cap} L {right} {y_cap} A {r} {r} 0 0 1 {x2} {shoulder} L {x2} {y_base} Z"
    )
  } else {
    let shoulder = y_cap - r;
    let (left, right) = (x + r, x2 - r);
    format!(
      "M {x} {y_base} L {x} {shoulder} A {r} {r} 0 0 0 {left} {y_cap} L {right} {y_cap} A {r} {r} 0 0 0 {x2} {shoulder} L {x2} {y_base} Z"
    )
  }
}

/// Per-slot marker shapes, in the same fixed order as the hues. This is the
/// secondary encoding `MAX_SCATTER_SERIES` refers to: on an all-pairs form
/// hue alone is not guaranteed to separate every pair under CVD, so shape
/// carries identity alongside it rather than instead of it.
fn marker_path(slot: usize, cx: f64, cy: f64, r: f64) -> String {
  match slot % MAX_SCATTER_SERIES {
    0 => format!(
      "M {} {cy} A {r} {r} 0 1 0 {} {cy} A {r} {r} 0 1 0 {} {cy} Z",
      cx - r,
      cx + r,
      cx - r
    ),
    1 => format!(
      "M {} {} H {} V {} H {} Z",
      cx - r,
      cy - r,
      cx + r,
      cy + r,
      cx - r
    ),
    2 => format!(
      "M {cx} {} L {} {} L {} {} Z",
      cy - r,
      cx + r,
      cy + r,
      cx - r,
      cy + r
    ),
    _ => format!(
      "M {cx} {} L {} {cy} L {cx} {} L {} {cy} Z",
      cy - r,
      cx + r,
      cy + r,
      cx - r
    ),
  }
}

/// What actually gets drawn, after the kind's own transform. Everything
/// downstream -- plot, tooltip, legend, table view -- reads `series` from
/// here, so a bar chart's tooltip reports the bucket mean it drew rather
/// than a raw sample the reader cannot see on the chart.
struct Prepared {
  series: Vec<ChartSeries>,
  /// Series the kind could not honestly carry, dropped from the tail.
  dropped: usize,
  /// The transform, stated plainly, when there was one worth disclosing.
  note: Option<String>,
}

fn prepare(kind: ChartKind, series: &[ChartSeries], plot_w: f64) -> Prepared {
  let cap = match kind {
    ChartKind::Scatter => MAX_SCATTER_SERIES,
    _ => MAX_SERIES,
  };
  let dropped = series.len().saturating_sub(cap);
  let kept: Vec<ChartSeries> = series.iter().take(cap).cloned().collect();

  match kind {
    ChartKind::Bar => {
      let t_min = kept
        .iter()
        .filter_map(|s| s.points.first().map(|p| p.0))
        .min()
        .unwrap_or(0);
      let t_max = kept
        .iter()
        .filter_map(|s| s.points.last().map(|p| p.0))
        .max()
        .unwrap_or(t_min + 1);
      let samples = kept.iter().map(|s| s.points.len()).max().unwrap_or(1);
      let buckets = bucket_count(plot_w, kept.len(), samples);
      let width = ((t_max - t_min).max(1) as f64 / buckets as f64).round() as i64;
      Prepared {
        series: bucket_series(&kept, t_min, t_max, buckets),
        dropped,
        note: Some(format!(
          "Mean per {} bucket. A bucket nothing was reported in is left empty, not drawn as zero.",
          format_duration(width.max(1))
        )),
      }
    }
    ChartKind::Scatter => {
      let densest = kept.iter().map(|s| s.points.len()).max().unwrap_or(0);
      if densest > MAX_SCATTER_MARKS {
        let stride = densest.div_ceil(MAX_SCATTER_MARKS);
        Prepared {
          series: kept
            .into_iter()
            .map(|s| ChartSeries {
              key: s.key,
              points: s.points.into_iter().step_by(stride).collect(),
            })
            .collect(),
          dropped,
          note: Some(format!(
            "Every {stride}th sample is drawn — the full range is too dense to plot one mark each. The table view is unstrided."
          )),
        }
      } else {
        Prepared {
          series: kept,
          dropped,
          note: None,
        }
      }
    }
    _ => Prepared {
      series: kept,
      dropped,
      note: None,
    },
  }
}

#[component]
pub fn TelemetryChart(
  series: Vec<ChartSeries>,
  kind: Option<ChartKind>,
  references: Option<Vec<ChartReference>>,
) -> Element {
  let kind = kind.unwrap_or_default();
  let references = references.unwrap_or_default();
  let mut show_table = use_signal(|| false);
  let mut hover_time = use_signal(|| None::<i64>);

  let plottable: Vec<ChartSeries> = series
    .iter()
    .filter(|s| !s.points.is_empty())
    .cloned()
    .collect();

  if plottable.is_empty() {
    return rsx! {
      div { class: "text-sm text-base-content/50 italic py-8 text-center",
        "No numeric telemetry points in this range yet."
      }
    };
  }

  let plot_w = CANVAS_W - MARGIN_LEFT - MARGIN_RIGHT;
  let plot_h = CANVAS_H - MARGIN_TOP - MARGIN_BOTTOM;

  let prepared = prepare(kind, &plottable, plot_w);
  let drawn = prepared.series.clone();
  // A bucket can come back empty for every series (all samples in one
  // bucket of a wide range is still one bucket), so re-check rather than
  // assuming the transform preserved plottability.
  let drawn: Vec<ChartSeries> = drawn.into_iter().filter(|s| !s.points.is_empty()).collect();
  if drawn.is_empty() {
    return rsx! {
      div { class: "text-sm text-base-content/50 italic py-8 text-center",
        "No numeric telemetry points in this range yet."
      }
    };
  }

  let t_min = drawn
    .iter()
    .filter_map(|s| s.points.first().map(|p| p.0))
    .min()
    .unwrap_or(0);
  let t_max = drawn
    .iter()
    .filter_map(|s| s.points.last().map(|p| p.0))
    .max()
    .unwrap_or(t_min + 1);
  let t_span = (t_max - t_min).max(1) as f64;

  let v_min_raw = drawn
    .iter()
    .flat_map(|s| s.points.iter().map(|p| p.1))
    .fold(f64::INFINITY, f64::min);
  let v_max_raw = drawn
    .iter()
    .flat_map(|s| s.points.iter().map(|p| p.1))
    .fold(f64::NEG_INFINITY, f64::max);

  // A threshold only means something next to the data it gates, so the ones
  // near enough to draw go on the axis; the rest are named in words below
  // rather than compressing the series into a flat line at the edge.
  let (drawn_refs, distant_refs) = partition_references(&references, v_min_raw, v_max_raw);

  // Area and bar measure from zero, so zero has to be on the axis -- and
  // the end zero anchors gets no padding, or the bars float above their own
  // baseline.
  let zero_baseline = kind.needs_zero_baseline();
  let ref_lo = drawn_refs
    .iter()
    .map(|r| r.value)
    .fold(f64::INFINITY, f64::min);
  let ref_hi = drawn_refs
    .iter()
    .map(|r| r.value)
    .fold(f64::NEG_INFINITY, f64::max);
  let lo = v_min_raw.min(ref_lo);
  let hi = v_max_raw.max(ref_hi);
  let lo = if zero_baseline { lo.min(0.0) } else { lo };
  let hi = if zero_baseline { hi.max(0.0) } else { hi };
  let pad = ((hi - lo).abs() * 0.1).max(1.0);
  let v_min = if zero_baseline && lo == 0.0 {
    0.0
  } else {
    lo - pad
  };
  let v_max = if zero_baseline && hi == 0.0 {
    0.0
  } else {
    hi + pad
  };
  let v_span = (v_max - v_min).max(f64::EPSILON);

  let x_of = move |t: i64| MARGIN_LEFT + ((t - t_min) as f64 / t_span) * plot_w;
  let y_of = move |v: f64| MARGIN_TOP + (1.0 - (v - v_min) / v_span) * plot_h;
  let y_zero = y_of(0.0);

  let y_step = nice_step((hi - lo) / 3.0);
  let first_tick = (v_min / y_step).ceil() * y_step;
  let mut y_ticks = Vec::new();
  let mut tick = first_tick;
  while tick <= v_max {
    y_ticks.push(tick);
    tick += y_step;
  }

  let show_legend = drawn.len() >= 2;
  let dropped = prepared.dropped;
  let transform_note = prepared.note.clone();

  // Bars share each bucket slot between the series, leaving the slot's
  // outer fifth as air and a surface gap between neighbours.
  let bucket_slot = plot_w / (drawn.iter().map(|s| s.points.len()).max().unwrap_or(1) as f64);
  let band = bucket_slot * 0.8;
  let bar_w = ((band - SURFACE_GAP * (drawn.len().saturating_sub(1)) as f64) / drawn.len() as f64)
    .clamp(0.5, MAX_BAR_WIDTH);

  let hover_series = drawn.clone();
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
                for s in drawn.iter() {
                  th { "{s.key}" }
                }
              }
            }
            tbody {
              for (t , _) in drawn[0].points.iter() {
                tr {
                  td { class: "font-mono text-xs", "{format_time(*t)}" }
                  for s in drawn.iter() {
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

            // The zero line a bar's length or an area's fill is measured
            // against -- drawn only when it isn't already the axis, so the
            // reader can see where the measurement starts.
            if zero_baseline && y_zero < CANVAS_H - MARGIN_BOTTOM - 1.0 {
              line {
                x1: "{MARGIN_LEFT}",
                x2: "{CANVAS_W - MARGIN_RIGHT}",
                y1: "{y_zero}",
                y2: "{y_zero}",
                stroke: "var(--chart-axis)",
                stroke_width: "1",
              }
            }

            // Alert thresholds. Dashed, so they read as a boundary rather
            // than as more chrome, and drawn under the series so data is
            // never hidden behind its own threshold.
            for (i , r) in drawn_refs.iter().enumerate() {
              g { key: "threshold-{i}",
                line {
                  x1: "{MARGIN_LEFT}",
                  x2: "{CANVAS_W - MARGIN_RIGHT}",
                  y1: "{y_of(r.value)}",
                  y2: "{y_of(r.value)}",
                  stroke: if r.firing { "var(--chart-status-critical)" } else { "var(--chart-ink-secondary)" },
                  stroke_width: "1.5",
                  stroke_dasharray: "5 4",
                }
                text {
                  x: "{CANVAS_W - MARGIN_RIGHT - 2.0}",
                  y: "{(y_of(r.value) - 4.0).max(MARGIN_TOP + 8.0)}",
                  text_anchor: "end",
                  font_size: "9",
                  fill: "var(--chart-ink-secondary)",
                  "{r.label}"
                }
              }
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

            for (i , s) in drawn.iter().enumerate() {
              g { key: "{s.key}", class: "{series_color_class(i)}",
                match kind {
                    ChartKind::Bar => rsx! {
                      for (t , v) in s.points.iter() {
                        path {
                          key: "{t}",
                          d: bar_path(
                              x_of(*t) - band / 2.0 + i as f64 * (bar_w + SURFACE_GAP),
                              bar_w,
                              y_zero,
                              y_of(*v),
                          ),
                          fill: "currentColor",
                        }
                      }
                    },
                    ChartKind::Scatter => rsx! {
                      for (t , v) in s.points.iter() {
                        path {
                          key: "{t}",
                          d: marker_path(i, x_of(*t), y_of(*v), 3.5),
                          fill: "currentColor",
                          stroke: "var(--chart-surface)",
                          stroke_width: "{SURFACE_GAP}",
                        }
                      }
                    },
                    _ => {
                        let geometry = if kind == ChartKind::Step {
                            step_geometry(&s.points)
                        } else {
                            s.points.clone()
                        };
                        let line_points = geometry
                            .iter()
                            .map(|(t, v)| format!("{},{}", x_of(*t), y_of(*v)))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let area_points = geometry
                            .first()
                            .zip(geometry.last())
                            .map(|(first, last)| {
                                format!(
                                    "{},{} {} {},{}",
                                    x_of(first.0),
                                    y_zero,
                                    line_points,
                                    x_of(last.0),
                                    y_zero,
                                )
                            });
                        rsx! {
                          if kind == ChartKind::Area {
                            if let Some(points) = area_points {
                              polygon {
                                points: "{points}",
                                fill: "currentColor",
                                fill_opacity: "0.1",
                                stroke: "none",
                              }
                            }
                          }
                          polyline {
                            points: "{line_points}",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                          }
                          if let Some((t , v)) = s.points.last() {
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
                          }
                        }
                    }
                }
                // Single series carries no legend, so its identity and its
                // latest value ride the mark itself instead.
                if !show_legend {
                  if let Some((t , v)) = s.points.last() {
                    text {
                      x: "{(x_of(*t) + 8.0).min(CANVAS_W - MARGIN_RIGHT)}",
                      y: "{y_of(*v) - 8.0}",
                      text_anchor: if kind == ChartKind::Bar { "middle" } else { "start" },
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
              for (i , s) in drawn.iter().enumerate() {
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
          for (i , s) in drawn.iter().enumerate() {
            div { key: "{s.key}", class: "flex items-center gap-1.5 text-xs text-base-content/70",
              if kind == ChartKind::Scatter {
                svg {
                  width: "10",
                  height: "10",
                  view_box: "0 0 10 10",
                  class: "{series_color_class(i)}",
                  path { d: marker_path(i, 5.0, 5.0, 4.0), fill: "currentColor" }
                }
              } else if kind == ChartKind::Bar {
                span { class: "inline-block w-2.5 h-2.5 rounded-[2px] {series_color_class(i)} bg-current" }
              } else {
                span { class: "inline-block w-3 h-0.5 {series_color_class(i)} bg-current" }
              }
              "{s.key}"
            }
          }
        }
      }

      if let Some(note) = transform_note {
        div { class: "text-[11px] text-base-content/50", "{note}" }
      }

      for (i , r) in distant_refs.iter().enumerate() {
        div { key: "distant-{i}", class: "text-[11px] text-base-content/50",
          "{r.label} sits at {format_value(r.value)}, too far outside this range to plot without flattening it."
        }
      }

      if dropped > 0 {
        div { class: "text-[11px] text-base-content/50",
          if kind == ChartKind::Scatter {
            "+{dropped} more key(s) not shown — scatter puts every series on one plane, so it carries at most {MAX_SCATTER_SERIES} distinguishable ones. Line or step will show them all."
          } else {
            "+{dropped} more key(s) not shown — pick fewer to compare them."
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{ChartKind, ChartSeries, bucket_count, bucket_series, prepare, step_geometry};

  fn series(key: &str, points: &[(i64, f64)]) -> ChartSeries {
    ChartSeries {
      key: key.to_string(),
      points: points.to_vec(),
    }
  }

  #[test]
  fn step_geometry_holds_each_value_until_the_next_sample() {
    let held = step_geometry(&[(0, 1.0), (10, 5.0), (20, 2.0)]);
    assert_eq!(
      held,
      vec![(0, 1.0), (10, 1.0), (10, 5.0), (20, 5.0), (20, 2.0)]
    );
  }

  #[test]
  fn step_geometry_of_a_single_sample_is_that_sample() {
    assert_eq!(step_geometry(&[(7, 3.0)]), vec![(7, 3.0)]);
  }

  /// The whole reason bars are bucketed: a gap in reporting must not
  /// become a bar of height zero, which reads as a measurement.
  #[test]
  fn bucket_series_leaves_unreported_buckets_empty() {
    let bucketed = bucket_series(&[series("k", &[(0, 10.0), (90, 20.0)])], 0, 100, 10);
    let times: Vec<i64> = bucketed[0].points.iter().map(|p| p.0).collect();
    assert_eq!(times.len(), 2);
    assert_eq!(bucketed[0].points[0].1, 10.0);
    assert_eq!(bucketed[0].points[1].1, 20.0);
  }

  #[test]
  fn bucket_series_averages_samples_sharing_a_bucket() {
    let bucketed = bucket_series(&[series("k", &[(0, 10.0), (1, 20.0), (2, 30.0)])], 0, 10, 1);
    assert_eq!(bucketed[0].points.len(), 1);
    assert_eq!(bucketed[0].points[0].1, 20.0);
  }

  #[test]
  fn bucket_count_shrinks_as_series_are_added() {
    let one = bucket_count(580.0, 1, 5000);
    let six = bucket_count(580.0, 6, 5000);
    assert!(six < one, "{six} should be fewer than {one}");
    assert!(six >= 1);
  }

  #[test]
  fn bucket_count_never_exceeds_the_samples_it_has() {
    assert_eq!(bucket_count(580.0, 1, 3), 3);
  }

  /// Saved graphs can name more keys than an all-pairs form can keep
  /// distinguishable; the extras are dropped and counted, never recoloured
  /// out of a ninth hue.
  #[test]
  fn prepare_caps_scatter_series_and_reports_the_drop() {
    let many: Vec<ChartSeries> = (0..7)
      .map(|i| series(&format!("k{i}"), &[(0, 1.0), (10, 2.0)]))
      .collect();
    let prepared = prepare(ChartKind::Scatter, &many, 580.0);
    assert_eq!(prepared.series.len(), super::MAX_SCATTER_SERIES);
    assert_eq!(prepared.dropped, 7 - super::MAX_SCATTER_SERIES);
  }

  #[test]
  fn prepare_leaves_line_series_untouched() {
    let raw = vec![series("k", &[(0, 1.0), (10, 2.0), (20, 3.0)])];
    let prepared = prepare(ChartKind::Line, &raw, 580.0);
    assert_eq!(prepared.series, raw);
    assert_eq!(prepared.dropped, 0);
    assert!(prepared.note.is_none());
  }

  #[test]
  fn prepare_discloses_the_bucket_width_it_chose_for_bars() {
    let raw = vec![series("k", &[(0, 1.0), (3600, 2.0)])];
    let prepared = prepare(ChartKind::Bar, &raw, 580.0);
    assert!(
      prepared.note.is_some_and(|n| n.contains("Mean per")),
      "bar charts must state their bucket width"
    );
  }
}
