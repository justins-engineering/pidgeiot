// User-defined telemetry graphs. A GraphDef says which key(s) to
// plot, over what time range; GraphCard fetches the data and renders it with
// TelemetryChart. Backed by the real capsules::TelemetryHistoryPoint
// route shapes (api/telemetry.rs) — when a route call fails outright (route
// missing, network error), GraphCard falls back to clearly-labeled
// deterministic mock data so the widget is still usable to look at and
// review; a real pigeon that's just quiet gets an honest empty state
// instead (see `SeriesOutcome`).
use crate::LocalSession;
use crate::api::telemetry;
use crate::components::telemetry_chart::format_duration;
use crate::components::{ChartKind, ChartSeries, TelemetryChart};
use crate::helpers::graph_store::{self, GraphScope};
use crate::helpers::{connection_state, gps_track, is_page_hidden, sleep_ms};
use capsules::{TelemetryHistoryPoint, TelemetryLatest};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdRefreshCw;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

/// One saved graph. Persisted client-side only for now; see
/// `helpers::graph_store` for where that is, why, and what a move to the
/// backend would and would not involve.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GraphDef {
  pub id: String,
  pub title: String,
  /// Pigeon scope: one series per key. Flock scope: exactly one key,
  /// plotted as one series per pigeon in the flock (see `GraphScope::Flock`
  /// handling in `fetch_series`).
  pub keys: Vec<String>,
  pub range: TimeRange,
  /// `#[serde(default)]` is load-bearing rather than tidy: a graph saved
  /// before this field existed carries no `kind`, and without the default
  /// its whole scope fails to deserialize and reads back as no graphs at
  /// all. `Line` is the default precisely because it is what those graphs
  /// were already being drawn as.
  #[serde(default)]
  pub kind: ChartKind,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
  Last1h,
  Last6h,
  Last24h,
  Last7d,
  Last30d,
}

impl TimeRange {
  // `pub` (not module-private): `components::track_widget` reuses this
  // exact same range enum/dropdown pattern for the GPS track widget's own
  // time-range selector, per its own module doc comment.
  pub const ALL: [TimeRange; 5] = [
    TimeRange::Last1h,
    TimeRange::Last6h,
    TimeRange::Last24h,
    TimeRange::Last7d,
    TimeRange::Last30d,
  ];

  pub fn seconds(self) -> i64 {
    match self {
      TimeRange::Last1h => 3_600,
      TimeRange::Last6h => 6 * 3_600,
      TimeRange::Last24h => 24 * 3_600,
      TimeRange::Last7d => 7 * 24 * 3_600,
      TimeRange::Last30d => 30 * 24 * 3_600,
    }
  }

  pub fn label(self) -> &'static str {
    match self {
      TimeRange::Last1h => "Last hour",
      TimeRange::Last6h => "Last 6 hours",
      TimeRange::Last24h => "Last 24 hours",
      TimeRange::Last7d => "Last 7 days",
      TimeRange::Last30d => "Last 30 days",
    }
  }

  pub fn from_label(label: &str) -> Option<TimeRange> {
    TimeRange::ALL.into_iter().find(|r| r.label() == label)
  }
}

fn now() -> OffsetDateTime {
  OffsetDateTime::now_utc()
}

/// The reporting cadence at or above which a straight line between two
/// samples is claiming more than the data supports. Telemetry is sampled,
/// not continuous: at 30-second reporting the gaps are short enough that
/// reading a line as "roughly what it was doing" is fair, but at five
/// minutes and up the interpolation is drawing minutes of movement nobody
/// measured, and a step -- which holds the last reading until the next one
/// arrives -- is the shape that says only what was actually reported.
const STEP_RECOMMENDED_INTERVAL_SECS: i64 = 300;

/// Suggests a chart kind for a *new* graph, with the reason to show the
/// user. Deliberately separate from `ChartKind::default()`, which is pinned
/// to `Line` for deserializing already-saved graphs: this is advice at
/// creation time, and it comes with its evidence rather than quietly
/// preselecting something surprising.
///
/// Only cadence is used, because cadence is the one thing we actually know
/// about a key before plotting it. `None` (no configured interval) yields
/// no recommendation at all -- guessing from a key's name would be
/// inventing a signal.
fn recommended_kind(interval_secs: Option<i64>) -> Option<(ChartKind, String)> {
  let secs = interval_secs.filter(|s| *s >= STEP_RECOMMENDED_INTERVAL_SECS)?;
  Some((
    ChartKind::Step,
    format!(
      "Step is preselected: this pigeon reports every {}, so a line between two readings would draw {} of movement nobody measured.",
      format_duration(secs),
      format_duration(secs)
    ),
  ))
}

/// Deterministic per-key pseudo-random walk so the same key always renders
/// the same preview shape across re-renders (no visible flicker) without
/// pulling in a real RNG crate for what's explicitly placeholder data.
fn mock_points(key: &str, since: i64, until: i64) -> Vec<(i64, f64)> {
  let seed = key
    .bytes()
    .fold(7u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
  let mut state = seed;
  let steps = 24i64;
  let step_secs = ((until - since).max(1)) / steps;
  let mut value = 40.0 + (seed % 40) as f64;

  (0..=steps)
    .map(|i| {
      state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
      let noise = ((state >> 33) % 1000) as f64 / 1000.0 - 0.5;
      value += noise * 4.0;
      (since + step_secs * i, value)
    })
    .collect()
}

/// `telemetry::get_history`/`get_flock_history` return `None` only when the
/// fetch itself failed (route missing, network error, bad JSON — see
/// `fetch_json` in api/helpers.rs, which collapses all of those to `None`);
/// a real pigeon with no telemetry yet still comes back `Some(vec![])`. The
/// two must not be conflated: `Empty` gets TelemetryChart's own honest
/// empty-range message, `Preview` gets the mock-data disclaimer — showing
/// fabricated curves on a real, just-quiet pigeon would be actively
/// misleading.
enum SeriesOutcome {
  Live {
    series: Vec<ChartSeries>,
    /// The server hit its own per-response point cap, so this chart is
    /// showing the newest slice of a longer window than its range claims.
    /// `TelemetryHistory::truncated`'s "backend didn't say" case collapses
    /// to `false` here — the warning is only worth showing when we
    /// positively know the range was cut.
    truncated: bool,
  },
  Empty,
  Preview(Vec<ChartSeries>),
}

async fn fetch_series(source: &GraphScope, def: &GraphDef) -> SeriesOutcome {
  let until = now();
  let since = until - time::Duration::seconds(def.range.seconds());

  match source {
    GraphScope::Pigeon(pigeon_id) => match telemetry::get_history(pigeon_id, since, until).await {
      Some(history) if !history.points.is_empty() => SeriesOutcome::Live {
        series: series_from_history(&def.keys, &history.points),
        truncated: history.truncated.unwrap_or(false),
      },
      Some(_) => SeriesOutcome::Empty,
      None => SeriesOutcome::Preview(
        def
          .keys
          .iter()
          .map(|k| ChartSeries {
            key: k.clone(),
            points: mock_points(k, since.unix_timestamp(), until.unix_timestamp()),
          })
          .collect(),
      ),
    },
    GraphScope::Flock(flock_id) => {
      match telemetry::get_flock_history(flock_id, since, until).await {
        Some(history) if !history.points.is_empty() => SeriesOutcome::Live {
          series: series_from_flock_history(&def.keys, &history.points),
          truncated: history.truncated.unwrap_or(false),
        },
        Some(_) => SeriesOutcome::Empty,
        None => {
          let key = def.keys.first().cloned().unwrap_or_default();
          SeriesOutcome::Preview(
            (0..3)
              .map(|i| ChartSeries {
                key: format!("pigeon-{i}"),
                points: mock_points(
                  &format!("{key}{i}"),
                  since.unix_timestamp(),
                  until.unix_timestamp(),
                ),
              })
              .collect(),
          )
        }
      }
    }
  }
}

/// Pigeon-scoped rendering: one series per requested key. `points` already
/// carries `pigeon_id` (capsules' `TelemetryHistoryPoint` is shared with the
/// flock-scoped route, see api/telemetry.rs), but every row here is the
/// same pigeon so it's ignored — filtering by key alone is enough.
fn series_from_history(keys: &[String], points: &[TelemetryHistoryPoint]) -> Vec<ChartSeries> {
  keys
    .iter()
    .map(|k| {
      let mut pts: Vec<(i64, f64)> = points
        .iter()
        .filter(|p| &p.key == k)
        .filter_map(|p| p.value_num.map(|v| (p.reported_at.unix_timestamp(), v)))
        .collect();
      pts.sort_by_key(|p| p.0);
      ChartSeries {
        key: k.clone(),
        points: pts,
      }
    })
    .collect()
}

fn series_from_flock_history(
  keys: &[String],
  points: &[TelemetryHistoryPoint],
) -> Vec<ChartSeries> {
  let Some(key) = keys.first() else {
    return Vec::new();
  };
  let mut by_pigeon: BTreeMap<String, Vec<(i64, f64)>> = BTreeMap::new();
  for p in points.iter().filter(|p| &p.key == key) {
    if let Some(v) = p.value_num {
      by_pigeon
        .entry(p.pigeon_id.clone())
        .or_default()
        .push((p.reported_at.unix_timestamp(), v));
    }
  }
  by_pigeon
    .into_iter()
    .map(|(pid, mut pts)| {
      pts.sort_by_key(|p| p.0);
      ChartSeries {
        key: pid.chars().take(8).collect::<String>() + "…",
        points: pts,
      }
    })
    .collect()
}

/// Example keys shown while live telemetry (or a numeric subset of it)
/// isn't available yet -- see `is_mock_keys` below.
fn fallback_keys() -> Vec<String> {
  vec![
    "battery_mv".to_string(),
    "uptime_s".to_string(),
    "rssi_dbm".to_string(),
  ]
}

/// Telemetry keys with at least one numeric sample -- a non-numeric-valued
/// key (e.g. a firmware version string) can't be plotted as a line series;
/// `series_from_history`/`series_from_flock_history` above already drop
/// non-numeric points via `value_num`, so a key with none would otherwise
/// be pickable in `AddGraphModal` and render an empty chart. Mirrors
/// alerts_panel.rs's `numeric_keys_from_latest`/`numeric_keys_from_history`
/// (see CLAUDE.md's telemetry-forwarding note).
///
/// Also drops `gps_lat`/`gps_lon` specifically even though both parse as
/// perfectly numeric floats -- see `gps_track::is_line_graph_excluded`'s
/// own doc comment for why a raw absolute coordinate is a useless line
/// series (the GPS track widget is the right visualization for those
/// two). Every other gps_* key (altitude/speed/heading/sats/fix quality)
/// is an ordinary scalar and stays pickable here.
fn numeric_keys_from_latest(latest: &[TelemetryLatest]) -> Vec<String> {
  latest
    .iter()
    .filter(|l| l.value.trim().parse::<f64>().is_ok())
    .filter(|l| !gps_track::is_line_graph_excluded(&l.key))
    .map(|l| l.key.clone())
    .collect()
}

fn numeric_keys_from_history(points: &[TelemetryHistoryPoint]) -> Vec<String> {
  let mut keys: Vec<String> = points
    .iter()
    .filter(|p| p.value_num.is_some())
    .filter(|p| !gps_track::is_line_graph_excluded(&p.key))
    .map(|p| p.key.clone())
    .collect();
  keys.sort();
  keys.dedup();
  keys
}

#[component]
pub fn PigeonGraphs(
  pigeon_id: String,
  /// This pigeon's own `telemetry_interval` (seconds), already extracted by
  /// the caller from its shadow -- threaded straight into each `GraphCard`'s
  /// auto-refresh cadence via `connection_state::poll_interval_ms` rather
  /// than re-deriving it here from a shadow this component doesn't have.
  interval_secs: Option<i64>,
  /// One-click "add a graph" inbox for sibling widgets on the same page
  /// (currently `components::track_widget::TrackWidget`'s "+ Speed
  /// graph"/"+ Altitude graph" buttons) -- since `graphs` below is a
  /// `localStorage`-backed signal owned entirely by this component, a
  /// sibling can't push into it directly; the caller (`PigeonView`) wires
  /// this Signal to both components so a write here is picked up
  /// reactively instead of requiring a page reload to see the new graph.
  mut quick_add: Signal<Option<GraphDef>>,
  /// This pigeon's configured `telemetry_endpoint` URL, if it has one.
  /// Reports from such a pigeon are forwarded there *instead of* being
  /// written to the platform's own history table — the two paths are
  /// mutually exclusive per report — so its graphs are empty permanently
  /// rather than "not yet". The empty state names that cause instead of
  /// implying the device has gone quiet.
  forwarding_to: Option<String>,
) -> Element {
  let scope = GraphScope::Pigeon(pigeon_id.clone());
  let mut graphs = use_signal({
    let scope = scope.clone();
    move || graph_store::load(&scope)
  });
  let mut show_add = use_signal(|| false);
  let mut available_keys: Signal<Vec<String>> = use_signal(Vec::new);
  let mut is_mock_keys = use_signal(|| false);

  {
    let scope = scope.clone();
    use_effect(move || {
      if let Some(def) = quick_add() {
        // Idempotent: clicking "+ Speed graph" twice shouldn't create two
        // near-identical graphs -- a graph already covering this exact
        // key set is left alone rather than duplicated.
        if !graphs.read().iter().any(|g| g.keys == def.keys) {
          graphs.write().push(def);
          graph_store::save(&scope, &graphs.read());
        }
        quick_add.set(None);
      }
    });
  }

  {
    let pigeon_id = pigeon_id.clone();
    use_resource(move || {
      let pigeon_id = pigeon_id.clone();
      async move {
        match telemetry::get_latest(&pigeon_id).await {
          Some(latest) if !latest.is_empty() => {
            let keys = numeric_keys_from_latest(&latest);
            if keys.is_empty() {
              available_keys.set(fallback_keys());
              is_mock_keys.set(true);
            } else {
              available_keys.set(keys);
              is_mock_keys.set(false);
            }
          }
          _ => {
            available_keys.set(fallback_keys());
            is_mock_keys.set(true);
          }
        }
      }
    });
  }

  rsx! {
    div { class: "w-full flex flex-col gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold", "Telemetry" }
        button {
          class: "btn btn-secondary",
          onclick: move |_| show_add.set(true),
          "Add Graph"
        }
      }

      if is_mock_keys() {
        p { class: "text-xs text-warning/80 md:px-4",
          "No live telemetry reported for this pigeon yet — key picker is showing example keys."
        }
      }

      if graphs.read().is_empty() {
        p { class: "text-sm text-base-content/50 italic md:px-4",
          "No graphs yet. Add one to start tracking telemetry over time."
        }
      }

      div { class: "flex flex-col gap-6",
        for graph in graphs.read().iter().cloned() {
          GraphCard {
            key: "{graph.id}-{graph.range:?}-{graph.keys.join(\",\")}",
            def: graph.clone(),
            source: scope.clone(),
            interval_secs,
            forwarding_to: forwarding_to.clone(),
            on_remove: {
                let scope = scope.clone();
                move |id: String| {
                    graphs.write().retain(|g| g.id != id);
                    graph_store::save(&scope, &graphs.read());
                }
            },
            on_update: {
                let scope = scope.clone();
                move |updated: GraphDef| {
                    if let Some(g) = graphs.write().iter_mut().find(|g| g.id == updated.id) {
                        *g = updated;
                    }
                    graph_store::save(&scope, &graphs.read());
                }
            },
          }
        }
      }

      if show_add() {
        AddGraphModal {
          available_keys: available_keys(),
          multi_select: true,
          recommendation: recommended_kind(interval_secs),
          on_close: move |_| show_add.set(false),
          on_save: {
              let scope = scope.clone();
              move |def: GraphDef| {
                  graphs.write().push(def);
                  graph_store::save(&scope, &graphs.read());
                  show_add.set(false);
              }
          },
        }
      }
    }
  }
}

#[component]
pub fn FlockGraphs(flock_id: Uuid) -> Element {
  let scope = GraphScope::Flock(flock_id);
  let mut graphs = use_signal({
    let scope = scope.clone();
    move || graph_store::load(&scope)
  });
  let mut show_add = use_signal(|| false);
  let local_session = use_context::<LocalSession>();

  // No flock-level "latest keys" route — derive a best-effort key list from
  // the flock's own history fetch at the default range instead of adding
  // another endpoint.
  let mut available_keys: Signal<Vec<String>> = use_signal(Vec::new);
  let mut is_mock_keys = use_signal(|| false);
  use_resource(move || async move {
    let until = now();
    let since = until - time::Duration::seconds(TimeRange::Last24h.seconds());
    match telemetry::get_flock_history(&flock_id, since, until).await {
      Some(history) if !history.points.is_empty() => {
        let keys = numeric_keys_from_history(&history.points);
        if keys.is_empty() {
          available_keys.set(fallback_keys());
          is_mock_keys.set(true);
        } else {
          available_keys.set(keys);
          is_mock_keys.set(false);
        }
      }
      _ => {
        available_keys.set(fallback_keys());
        is_mock_keys.set(true);
      }
    }
  });

  let _ = &local_session; // reserved for once pigeon names are resolvable per-flock here too.

  rsx! {
    div { class: "w-full flex flex-col gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        h2 { class: "text-3xl font-bold", "Flock Telemetry" }
        button {
          class: "btn btn-secondary",
          onclick: move |_| show_add.set(true),
          "Add Graph"
        }
      }

      if is_mock_keys() {
        p { class: "text-xs text-warning/80 md:px-4",
          "No live telemetry reported for this flock yet — key picker is showing example keys."
        }
      }

      if graphs.read().is_empty() {
        p { class: "text-sm text-base-content/50 italic md:px-4",
          "No graphs yet. Add one to compare a metric across the flock's pigeons."
        }
      }

      div { class: "flex flex-col gap-6",
        for graph in graphs.read().iter().cloned() {
          GraphCard {
            key: "{graph.id}-{graph.range:?}-{graph.keys.join(\",\")}",
            def: graph.clone(),
            source: scope.clone(),
            // No single pigeon interval at flock scope -- falls back to
            // poll_interval_ms's fixed default. Nor one telemetry endpoint:
            // forwarding is per-pigeon, and a flock chart would need every
            // member's endpoint to say anything true about why it's empty.
            interval_secs: None,
            forwarding_to: None,
            on_remove: {
                let scope = scope.clone();
                move |id: String| {
                    graphs.write().retain(|g| g.id != id);
                    graph_store::save(&scope, &graphs.read());
                }
            },
            on_update: {
                let scope = scope.clone();
                move |updated: GraphDef| {
                    if let Some(g) = graphs.write().iter_mut().find(|g| g.id == updated.id) {
                        *g = updated;
                    }
                    graph_store::save(&scope, &graphs.read());
                }
            },
          }
        }
      }

      if show_add() {
        AddGraphModal {
          available_keys: available_keys(),
          multi_select: false,
          // No per-pigeon reporting cadence at flock scope, so nothing to
          // base a kind recommendation on -- see `recommended_kind`.
          recommendation: None,
          on_close: move |_| show_add.set(false),
          on_save: {
              let scope = scope.clone();
              move |def: GraphDef| {
                  graphs.write().push(def);
                  graph_store::save(&scope, &graphs.read());
                  show_add.set(false);
              }
          },
        }
      }
    }
  }
}

async fn refresh_outcome(
  source: &GraphScope,
  def: &GraphDef,
  mut outcome: Signal<Option<SeriesOutcome>>,
  mut loading: Signal<bool>,
) {
  loading.set(true);
  let fresh = fetch_series(source, def).await;
  // `Preview` only ever means the fetch itself failed (see `fetch_series` --
  // a real "no telemetry yet" pigeon comes back `Empty`, not `Preview`), so
  // a `Preview` result on a poll that follows a real `Live`/`Empty` fetch is
  // a transient failure, not new information. Downgrading the chart to mock
  // curves in that case is exactly the "actively misleading" outcome this
  // module's own `SeriesOutcome` doc comment warns against -- keep showing
  // the last good state and let the next poll retry, same as a dropped
  // request the user never has to know about. `peek()`, not `read()|()`: an
  // untracked read so this async fn (spawned by `use_future`) never
  // subscribes to the very signal it's about to write, which would
  // otherwise restart the polling loop on its own `set()` below.
  let has_prior_data = matches!(
    outcome.peek().as_ref(),
    Some(SeriesOutcome::Live { .. }) | Some(SeriesOutcome::Empty)
  );
  if !(matches!(fresh, SeriesOutcome::Preview(_)) && has_prior_data) {
    outcome.set(Some(fresh));
  }
  loading.set(false);
}

#[component]
fn GraphCard(
  def: GraphDef,
  source: GraphScope,
  interval_secs: Option<i64>,
  forwarding_to: Option<String>,
  on_remove: EventHandler<String>,
  on_update: EventHandler<GraphDef>,
) -> Element {
  let outcome: Signal<Option<SeriesOutcome>> = use_signal(|| None);
  let loading = use_signal(|| true);

  // Fetches immediately on mount, then keeps re-fetching at this pigeon's
  // own self-calibrated cadence for as long as the card stays mounted
  // (Dioxus cancels the future on unmount, same as `views::demo`'s
  // equivalent loop) -- skips the fetch entirely while the tab is
  // backgrounded (`is_page_hidden`) so a dashboard nobody is looking at
  // doesn't keep polling the Durable Object. A graph's `id`/`range`/`keys`
  // changing at all already remounts this component fresh (see the `key`
  // passed at each call site), so a plain loop over `def`/`source` captured
  // at mount is enough for those -- `source`'s pigeon/flock id is otherwise
  // constant for the lifetime of a mounted parent anyway. `interval_secs`
  // is NOT part of that key, though: see `poll_interval_ms`'s own doc
  // comment for the one case this loop doesn't react to (a shadow's
  // `telemetry_interval` changing mid-visit keeps the cadence this loop
  // already started with).
  {
    let def = def.clone();
    let source = source.clone();
    use_future(move || {
      let def = def.clone();
      let source = source.clone();
      async move {
        let poll_ms = connection_state::poll_interval_ms(interval_secs);
        loop {
          if !is_page_hidden() {
            refresh_outcome(&source, &def, outcome, loading).await;
          }
          sleep_ms(poll_ms).await;
        }
      }
    });
  }

  rsx! {
    div { class: "border border-base-content/10 rounded-box p-4 flex flex-col gap-3",
      div { class: "flex items-center justify-between gap-2 flex-wrap",
        div {
          h3 { class: "font-semibold text-lg", "{def.title}" }
          p { class: "text-xs text-base-content/50", "{def.keys.join(\", \")}" }
        }
        div { class: "flex items-center gap-2 flex-wrap",
          select {
            class: "select select-bordered select-sm",
            title: "{def.kind.describes()}",
            "aria-label": "Chart type",
            value: "{def.kind.label()}",
            onchange: {
                let def = def.clone();
                move |evt: Event<FormData>| {
                    if let Some(kind) = ChartKind::from_label(&evt.value()) {
                        let mut updated = def.clone();
                        updated.kind = kind;
                        on_update.call(updated);
                    }
                }
            },
            for k in ChartKind::ALL {
              option { value: "{k.label()}", selected: k == def.kind, "{k.label()}" }
            }
          }
          select {
            class: "select select-bordered select-sm",
            "aria-label": "Time range",
            value: "{def.range.label()}",
            onchange: {
                let def = def.clone();
                move |evt: Event<FormData>| {
                    if let Some(range) = TimeRange::from_label(&evt.value()) {
                        let mut updated = def.clone();
                        updated.range = range;
                        on_update.call(updated);
                    }
                }
            },
            for r in TimeRange::ALL {
              option { value: "{r.label()}", selected: r == def.range, "{r.label()}" }
            }
          }
          button {
            class: "btn btn-ghost btn-sm",
            r#type: "button",
            title: "Refresh now",
            disabled: loading(),
            onclick: {
                let def = def.clone();
                let source = source.clone();
                move |_| {
                    let def = def.clone();
                    let source = source.clone();
                    async move {
                        refresh_outcome(&source, &def, outcome, loading).await;
                    }
                }
            },
            if loading() {
              span { class: "loading loading-spinner loading-xs" }
            } else {
              Icon { icon: LdRefreshCw, width: 14, height: 14 }
            }
          }
          button {
            class: "btn btn-ghost btn-sm text-error",
            r#type: "button",
            onclick: {
                let id = def.id.clone();
                move |_| on_remove.call(id.clone())
            },
            "Remove"
          }
        }
      }

      if loading() && outcome.read().is_none() {
        div { class: "loading loading-spinner loading-sm text-primary" }
      } else {
        match outcome.read().as_ref() {
          Some(SeriesOutcome::Preview(series)) => rsx! {
            p { class: "text-[11px] text-warning/80",
              "Preview data — showing example values until live telemetry history is available here."
            }
            TelemetryChart { series: series.clone(), kind: def.kind }
          },
          Some(SeriesOutcome::Live { series, truncated }) => rsx! {
            if *truncated {
              p { class: "text-[11px] text-warning/80",
                "Newest {capsules::TELEMETRY_HISTORY_MAX_POINTS} points only — this range holds more than one response can carry, so its earliest part isn't drawn. Pick a shorter range to see a complete window."
              }
            }
            TelemetryChart { series: series.clone(), kind: def.kind }
          },
          Some(SeriesOutcome::Empty) | None => match forwarding_to.as_ref() {
            Some(url) => rsx! {
              div { class: "text-sm text-base-content/60 py-8 text-center flex flex-col gap-1",
                p {
                  "No history to chart — this pigeon's telemetry is forwarded to "
                  span { class: "font-mono text-xs break-all", "{url}" }
                  " instead of being stored here."
                }
                p { class: "text-xs",
                  "Clear the telemetry endpoint to have PidgeIoT keep history for graphs."
                }
              }
            },
            None => rsx! {
              TelemetryChart { series: Vec::new() }
            },
          },
        }
      }
    }
  }
}

#[component]
fn AddGraphModal(
  available_keys: Vec<String>,
  multi_select: bool,
  /// A kind to preselect plus the evidence for it, from
  /// `recommended_kind`. `None` preselects the plain default and shows no
  /// justification, because there is nothing to justify.
  recommendation: Option<(ChartKind, String)>,
  on_close: EventHandler<()>,
  on_save: EventHandler<GraphDef>,
) -> Element {
  let mut title = use_signal(String::new);
  let mut selected_keys: Signal<Vec<String>> = use_signal(Vec::new);
  let mut range = use_signal(|| TimeRange::Last24h);
  let recommended = recommendation.as_ref().map(|(k, _)| *k);
  let recommendation_reason = recommendation.map(|(_, reason)| reason);
  let mut kind = use_signal(|| recommended.unwrap_or_default());
  let can_save = !title.read().trim().is_empty() && !selected_keys.read().is_empty();

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      onkeydown: move |e| {
          if e.key() == Key::Escape {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-md",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          onclick: move |_| on_close.call(()),
          "✕"
        }
        h3 { class: "text-lg font-bold mb-4", "Add Graph" }

        fieldset { class: "fieldset flex flex-col gap-4",
          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Title" }
            input {
              class: "input input-bordered w-full text-sm",
              r#type: "text",
              placeholder: "e.g., Battery over time",
              value: "{title}",
              oninput: move |e| title.set(e.value()),
            }
          }

          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1",
              if multi_select { "Keys (pick one or more)" } else { "Key (pick one)" }
            }
            div { class: "flex flex-col gap-1 max-h-40 overflow-y-auto",
              if available_keys.is_empty() {
                p { class: "text-xs text-base-content/50 italic", "No telemetry keys available yet." }
              }
              for k in available_keys.iter().cloned() {
                label { class: "flex items-center gap-2 text-sm cursor-pointer",
                  input {
                    r#type: if multi_select { "checkbox" } else { "radio" },
                    name: "graph-key",
                    checked: selected_keys.read().contains(&k),
                    onchange: {
                        let k = k.clone();
                        move |evt: Event<FormData>| {
                            let checked = evt.checked();
                            if multi_select {
                                let mut keys = selected_keys.write();
                                if checked {
                                    if !keys.contains(&k) {
                                        keys.push(k.clone());
                                    }
                                } else {
                                    keys.retain(|existing| existing != &k);
                                }
                            } else {
                                selected_keys.set(vec![k.clone()]);
                            }
                        }
                    },
                  }
                  "{k}"
                }
              }
            }
          }

          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Chart type" }
            select {
              class: "select select-bordered w-full text-sm",
              value: "{kind().label()}",
              onchange: move |evt: Event<FormData>| {
                  if let Some(k) = ChartKind::from_label(&evt.value()) {
                      kind.set(k);
                  }
              },
              for k in ChartKind::ALL {
                option { value: "{k.label()}", selected: k == kind(), "{k.label()}" }
              }
            }
            p { class: "text-xs text-base-content/60 mt-1", "{kind().describes()}" }
            if let Some(reason) = recommendation_reason.as_ref() {
              if Some(kind()) == recommended {
                p { class: "text-xs text-base-content/50 mt-1", "{reason}" }
              }
            }
          }

          div {
            label { class: "fieldset-legend text-xs font-semibold mb-1", "Time range" }
            select {
              class: "select select-bordered w-full text-sm",
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
        }

        div { class: "modal-action",
          button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
          button {
            class: "btn btn-primary",
            disabled: !can_save,
            onclick: move |_| {
                let def = GraphDef {
                    // Workspace uuid only enables v7 (js feature covers wasm Date.now)
                    id: uuid::Uuid::now_v7().to_string(),
                    title: title.read().clone(),
                    keys: selected_keys.read().clone(),
                    range: range(),
                    kind: kind(),
                };
                on_save.call(def);
            },
            "Save"
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{numeric_keys_from_history, numeric_keys_from_latest};
  use capsules::{TelemetryHistoryPoint, TelemetryLatest};
  use time::OffsetDateTime;

  fn latest(key: &str, value: &str) -> TelemetryLatest {
    TelemetryLatest {
      key: key.to_string(),
      value: value.to_string(),
      reported_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  fn history_point(key: &str, value: &str, value_num: Option<f64>) -> TelemetryHistoryPoint {
    TelemetryHistoryPoint {
      pigeon_id: "p1".to_string(),
      key: key.to_string(),
      value: value.to_string(),
      value_num,
      reported_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  #[test]
  fn numeric_keys_from_latest_excludes_non_numeric() {
    let latest = vec![
      latest("battery_mv", "3300"),
      latest("fw_version", "1.2.0"),
      latest("rssi_dbm", "-71.5"),
    ];
    assert_eq!(
      numeric_keys_from_latest(&latest),
      vec!["battery_mv", "rssi_dbm"]
    );
  }

  /// GPS device sample: `gps_lat`/`gps_lon` parse as perfectly valid
  /// floats (so the plain numeric filter alone wouldn't catch them) but
  /// are excluded as a dedicated line-graph-usefulness judgment call --
  /// see `gps_track::is_line_graph_excluded`'s doc comment. Every other
  /// gps_* key stays pickable since it's an ordinary scalar.
  #[test]
  fn numeric_keys_from_latest_excludes_gps_lat_lon_but_keeps_other_gps_keys() {
    let latest = vec![
      latest("gps_lat", "40.7128"),
      latest("gps_lon", "-74.0060"),
      latest("gps_speed_mps", "3.2"),
      latest("gps_alt_m", "12.5"),
      latest("gps_sats", "8"),
      latest("battery_mv", "3300"),
    ];
    // `numeric_keys_from_latest` preserves input order (unlike the
    // history variant below, which sorts/dedups) -- gps_lat/gps_lon are
    // simply dropped from wherever they sat in `latest`.
    assert_eq!(
      numeric_keys_from_latest(&latest),
      vec!["gps_speed_mps", "gps_alt_m", "gps_sats", "battery_mv"]
    );
  }

  #[test]
  fn numeric_keys_from_history_excludes_key_with_no_numeric_samples() {
    let points = vec![
      history_point("battery_mv", "3300", Some(3300.0)),
      history_point("fw_version", "1.2.0", None),
      history_point("fw_version", "1.2.1", None),
    ];
    assert_eq!(numeric_keys_from_history(&points), vec!["battery_mv"]);
  }

  #[test]
  fn numeric_keys_from_history_dedups_and_sorts() {
    let points = vec![
      history_point("uptime_s", "10", Some(10.0)),
      history_point("battery_mv", "3300", Some(3300.0)),
      history_point("uptime_s", "20", Some(20.0)),
    ];
    assert_eq!(
      numeric_keys_from_history(&points),
      vec!["battery_mv", "uptime_s"]
    );
  }

  #[test]
  fn numeric_keys_from_history_excludes_gps_lat_lon_but_keeps_other_gps_keys() {
    let points = vec![
      history_point("gps_lat", "40.7128", Some(40.7128)),
      history_point("gps_lon", "-74.0060", Some(-74.0060)),
      history_point("gps_heading_deg", "180", Some(180.0)),
      history_point("battery_mv", "3300", Some(3300.0)),
    ];
    assert_eq!(
      numeric_keys_from_history(&points),
      vec!["battery_mv", "gps_heading_deg"]
    );
  }
}
