// User-defined telemetry graphs (task #19). A GraphDef says which key(s) to
// plot, over what time range; GraphCard fetches the data and renders it with
// TelemetryChart. Backed by task #18's real capsules::TelemetryHistoryPoint
// route shapes (api/telemetry.rs) — when a route call fails outright (route
// missing, network error), GraphCard falls back to clearly-labeled
// deterministic mock data so the widget is still usable to look at and
// review; a real pigeon that's just quiet gets an honest empty state
// instead (see `SeriesOutcome`).
use crate::LocalSession;
use crate::api::telemetry;
use crate::components::{ChartSeries, TelemetryChart};
use crate::helpers::gps_track;
use crate::local_storage;
use capsules::{TelemetryHistoryPoint, TelemetryLatest};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

/// Persisted client-side only (localStorage v1, see local_storage.rs) —
/// deliberately NOT a server round trip yet. Server-side persistence (so a
/// user's graphs follow them across browsers) is a later upgrade once
/// there's a natural place to hang per-user dashboard config on the
/// Pigeon/Flock API; today capsules has nothing like it, and adding one is
/// out of scope for this pass (capsules is owned by the dovecote agent this
/// cycle).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GraphDef {
  pub id: String,
  pub title: String,
  /// Pigeon scope: one series per key. Flock scope: exactly one key,
  /// plotted as one series per pigeon in the flock (see `DataSource::Flock`
  /// handling in `fetch_series`).
  pub keys: Vec<String>,
  pub range: TimeRange,
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

#[derive(Clone, Debug, PartialEq)]
enum DataSource {
  Pigeon(String),
  Flock(Uuid),
}

fn now() -> OffsetDateTime {
  OffsetDateTime::now_utc()
}

fn storage_key(scope: &str, id: &str) -> String {
  format!("pidgeiot.graphs.v1.{scope}.{id}")
}

fn load_graphs(scope: &str, id: &str) -> Vec<GraphDef> {
  local_storage::load(&storage_key(scope, id)).unwrap_or_default()
}

fn save_graphs(scope: &str, id: &str, graphs: &[GraphDef]) {
  local_storage::save(&storage_key(scope, id), &graphs);
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
      state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
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
  Live(Vec<ChartSeries>),
  Empty,
  Preview(Vec<ChartSeries>),
}

async fn fetch_series(source: &DataSource, def: &GraphDef) -> SeriesOutcome {
  let until = now();
  let since = until - time::Duration::seconds(def.range.seconds());

  match source {
    DataSource::Pigeon(pigeon_id) => match telemetry::get_history(pigeon_id, since, until).await {
      Some(points) if !points.is_empty() => {
        SeriesOutcome::Live(series_from_history(&def.keys, &points))
      }
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
    DataSource::Flock(flock_id) => {
      match telemetry::get_flock_history(flock_id, since, until).await {
        Some(points) if !points.is_empty() => {
          SeriesOutcome::Live(series_from_flock_history(&def.keys, &points))
        }
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

fn series_from_flock_history(keys: &[String], points: &[TelemetryHistoryPoint]) -> Vec<ChartSeries> {
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
/// (task #32 point 4, see CLAUDE.md's telemetry-forwarding note).
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
  /// One-click "add a graph" inbox for sibling widgets on the same page
  /// (currently `components::track_widget::TrackWidget`'s "+ Speed
  /// graph"/"+ Altitude graph" buttons) -- since `graphs` below is a
  /// `localStorage`-backed signal owned entirely by this component, a
  /// sibling can't push into it directly; the caller (`PigeonView`) wires
  /// this Signal to both components so a write here is picked up
  /// reactively instead of requiring a page reload to see the new graph.
  mut quick_add: Signal<Option<GraphDef>>,
) -> Element {
  let mut graphs = use_signal(|| load_graphs("pigeon", &pigeon_id));
  let mut show_add = use_signal(|| false);
  let mut available_keys: Signal<Vec<String>> = use_signal(Vec::new);
  let mut is_mock_keys = use_signal(|| false);

  {
    let pigeon_id = pigeon_id.clone();
    use_effect(move || {
      if let Some(def) = quick_add() {
        // Idempotent: clicking "+ Speed graph" twice shouldn't create two
        // near-identical graphs -- a graph already covering this exact
        // key set is left alone rather than duplicated.
        if !graphs.read().iter().any(|g| g.keys == def.keys) {
          graphs.write().push(def);
          save_graphs("pigeon", &pigeon_id, &graphs.read());
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
            source: DataSource::Pigeon(pigeon_id.clone()),
            on_remove: {
                let pigeon_id = pigeon_id.clone();
                move |id: String| {
                    graphs.write().retain(|g| g.id != id);
                    save_graphs("pigeon", &pigeon_id, &graphs.read());
                }
            },
            on_update: {
                let pigeon_id = pigeon_id.clone();
                move |updated: GraphDef| {
                    if let Some(g) = graphs.write().iter_mut().find(|g| g.id == updated.id) {
                        *g = updated;
                    }
                    save_graphs("pigeon", &pigeon_id, &graphs.read());
                }
            },
          }
        }
      }

      if show_add() {
        AddGraphModal {
          available_keys: available_keys(),
          multi_select: true,
          on_close: move |_| show_add.set(false),
          on_save: {
              let pigeon_id = pigeon_id.clone();
              move |def: GraphDef| {
                  graphs.write().push(def);
                  save_graphs("pigeon", &pigeon_id, &graphs.read());
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
  let scope_id = flock_id.to_string();
  let mut graphs = use_signal(|| load_graphs("flock", &scope_id));
  let mut show_add = use_signal(|| false);
  let local_session = use_context::<LocalSession>();

  // No flock-level "latest keys" route — derive a best-effort key list from
  // the flock's own history fetch at the default range instead of adding
  // another endpoint on top of the ones task #18 already owns.
  let mut available_keys: Signal<Vec<String>> = use_signal(Vec::new);
  let mut is_mock_keys = use_signal(|| false);
  use_resource(move || async move {
    let until = now();
    let since = until - time::Duration::seconds(TimeRange::Last24h.seconds());
    match telemetry::get_flock_history(&flock_id, since, until).await {
      Some(points) if !points.is_empty() => {
        let keys = numeric_keys_from_history(&points);
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
            source: DataSource::Flock(flock_id),
            on_remove: {
                let scope_id = scope_id.clone();
                move |id: String| {
                    graphs.write().retain(|g| g.id != id);
                    save_graphs("flock", &scope_id, &graphs.read());
                }
            },
            on_update: {
                let scope_id = scope_id.clone();
                move |updated: GraphDef| {
                    if let Some(g) = graphs.write().iter_mut().find(|g| g.id == updated.id) {
                        *g = updated;
                    }
                    save_graphs("flock", &scope_id, &graphs.read());
                }
            },
          }
        }
      }

      if show_add() {
        AddGraphModal {
          available_keys: available_keys(),
          multi_select: false,
          on_close: move |_| show_add.set(false),
          on_save: {
              let scope_id = scope_id.clone();
              move |def: GraphDef| {
                  graphs.write().push(def);
                  save_graphs("flock", &scope_id, &graphs.read());
                  show_add.set(false);
              }
          },
        }
      }
    }
  }
}

#[component]
fn GraphCard(
  def: GraphDef,
  source: DataSource,
  on_remove: EventHandler<String>,
  on_update: EventHandler<GraphDef>,
) -> Element {
  let mut outcome: Signal<Option<SeriesOutcome>> = use_signal(|| None);
  let mut loading = use_signal(|| true);

  {
    let def = def.clone();
    let source = source.clone();
    use_resource(move || {
      let def = def.clone();
      let source = source.clone();
      async move {
        loading.set(true);
        outcome.set(Some(fetch_series(&source, &def).await));
        loading.set(false);
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
        div { class: "flex items-center gap-2",
          select {
            class: "select select-bordered select-sm",
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
            TelemetryChart { series: series.clone() }
          },
          Some(SeriesOutcome::Live(series)) => rsx! {
            TelemetryChart { series: series.clone() }
          },
          Some(SeriesOutcome::Empty) | None => rsx! {
            TelemetryChart { series: Vec::new() }
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
  on_close: EventHandler<()>,
  on_save: EventHandler<GraphDef>,
) -> Element {
  let mut title = use_signal(String::new);
  let mut selected_keys: Signal<Vec<String>> = use_signal(Vec::new);
  let mut range = use_signal(|| TimeRange::Last24h);
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
