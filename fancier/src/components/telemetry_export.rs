//! The "Export CSV" panel of the pigeon and flock telemetry sections. It walks the raw history
//! route backwards a page at a time (`helpers::telemetry_export` plans each step) and saves one
//! CSV file, telling the user before the save when the range held more than it will fetch.

use super::graph_widget::TimeRange;
use crate::LocalSession;
use crate::api::{pigeons, telemetry};
use crate::components::GraphDef;
use crate::helpers::download_text_parts;
use crate::helpers::graph_store::GraphScope;
use crate::helpers::telemetry_export::{self as export, CSV_HEADER, PageStep};
use capsules::TELEMETRY_HISTORY_MAX_POINTS;
use dioxus::prelude::*;
use std::collections::HashMap;
use time::OffsetDateTime;
use wasm_bindgen::JsValue;

/// The range the panel opens with: the widest one a graph on screen shows, else a day.
fn default_range(graphs: &[GraphDef]) -> TimeRange {
  graphs
    .iter()
    .map(|g| g.range)
    .max_by_key(|r| r.seconds())
    .unwrap_or(TimeRange::Last24h)
}

/// The keys the panel opens with: every key a graph on screen draws. Empty means all keys.
fn default_keys(graphs: &[GraphDef]) -> Vec<String> {
  key_choices(&[], graphs)
}

/// The keys offered by name: those the section knows were reported plus those its graphs draw,
/// sorted, without the ones only "All keys" can reach.
fn key_choices(reported: &[String], graphs: &[GraphDef]) -> Vec<String> {
  let mut keys: Vec<String> = reported
    .iter()
    .chain(graphs.iter().flat_map(|g| g.keys.iter()))
    .filter(|k| export::filterable_key(k))
    .cloned()
    .collect();
  keys.sort();
  keys.dedup();
  keys
}

/// Everything one walk needs, fixed when it starts. [`start`] fills in the names.
#[derive(Clone)]
struct Request {
  scope: GraphScope,
  /// `None` exports every key.
  keys: Option<Vec<String>>,
  since: OffsetDateTime,
  until: OffsetDateTime,
  pigeon_names: HashMap<String, String>,
  flock_name: String,
  /// What the file is named after: the pigeon's or the flock's name.
  scope_name: String,
}

impl Request {
  fn file_name(&self, since: OffsetDateTime) -> String {
    let fallback = match self.scope {
      GraphScope::Pigeon(_) => "pigeon",
      GraphScope::Flock(_) => "flock",
    };
    export::file_name(&self.scope_name, fallback, since, self.until)
  }
}

/// The rows fetched so far: one CSV string per page, newest page first.
struct Walked {
  parts: js_sys::Array,
  rows: usize,
  bytes: usize,
  oldest: Option<OffsetDateTime>,
  newest: Option<OffsetDateTime>,
}

enum WalkEnd {
  Complete(Walked),
  Capped(Walked, OffsetDateTime),
  Stalled(Option<OffsetDateTime>),
  Failed(usize),
  NoHeader,
}

async fn walk(request: &Request, mut progress: Signal<usize>) -> WalkEnd {
  let mut walked = Walked {
    parts: js_sys::Array::new(),
    rows: 0,
    bytes: 0,
    oldest: None,
    newest: None,
  };
  let mut until = request.until;
  let mut page_index = 0;
  loop {
    let Some(page) = telemetry::get_history_page(
      &request.scope,
      request.keys.as_deref(),
      request.since,
      until,
    )
    .await
    else {
      return WalkEnd::Failed(walked.rows);
    };
    let Some(truncated) = page.truncated else {
      return WalkEnd::NoHeader;
    };
    let mut points = page.points;
    let last_page =
      page_index + 1 >= export::EXPORT_MAX_PAGES || walked.bytes >= export::EXPORT_MAX_BYTES;
    let (keep_from, step) = export::plan_page(&points, truncated, last_page);
    let kept = &mut points[keep_from..];
    if let (Some(first), Some(last)) = (kept.first(), kept.last()) {
      walked.oldest = Some(first.reported_at);
      walked.newest.get_or_insert(last.reported_at);
      walked.rows += kept.len();
      let csv = export::rows_csv(kept, &request.pigeon_names, &request.flock_name);
      walked.bytes += csv.len();
      walked.parts.push(&JsValue::from_str(&csv));
      progress.set(walked.rows);
    }
    match step {
      PageStep::Done => return WalkEnd::Complete(walked),
      PageStep::Continue(next) => until = next,
      PageStep::Capped(left_out) => return WalkEnd::Capped(walked, left_out),
      PageStep::Stalled(at) => return WalkEnd::Stalled(at),
    }
    page_index += 1;
  }
}

enum Phase {
  Idle,
  Running,
  /// A capped walk waiting for the user to accept the newest part.
  Confirm {
    walked: Walked,
    left_out: OffsetDateTime,
    request: Request,
  },
  Saved {
    rows: usize,
    file: String,
    /// The walk for the points a capped file left out.
    older: Option<Request>,
  },
  Empty,
  Stalled(Option<OffsetDateTime>),
  Failed(usize),
  NoHeader,
  SaveFailed,
}

/// Header first, then the pages oldest first, into one file.
fn save(walked: Walked, request: &Request, since: OffsetDateTime, older: Option<Request>) -> Phase {
  let file = request.file_name(since);
  walked.parts.reverse();
  walked.parts.unshift(&JsValue::from_str(CSV_HEADER));
  match download_text_parts(&walked.parts, &file, "text/csv;charset=utf-8") {
    Some(()) => Phase::Saved {
      rows: walked.rows,
      file,
      older,
    },
    None => Phase::SaveFailed,
  }
}

fn start(
  mut request: Request,
  local: LocalSession,
  mut phase: Signal<Phase>,
  mut progress: Signal<usize>,
) {
  phase.set(Phase::Running);
  progress.set(0);
  spawn(async move {
    load_flock_pigeons(&request.scope, &local).await;
    (request.pigeon_names, request.flock_name, request.scope_name) = names(&request.scope, &local);
    let next = match walk(&request, progress).await {
      WalkEnd::Complete(walked) if walked.rows == 0 => Phase::Empty,
      WalkEnd::Complete(walked) => save(walked, &request, request.since, None),
      WalkEnd::Capped(walked, left_out) => Phase::Confirm {
        walked,
        left_out,
        request,
      },
      WalkEnd::Stalled(at) => Phase::Stalled(at),
      WalkEnd::Failed(rows) => Phase::Failed(rows),
      WalkEnd::NoHeader => Phase::NoHeader,
    };
    phase.set(next);
  });
}

/// Fetches the flock's pigeons the dashboard has not loaded yet, so an export started before the
/// flock page's own fetch lands still names them.
async fn load_flock_pigeons(scope: &GraphScope, local: &LocalSession) {
  let GraphScope::Flock(flock_id) = scope else {
    return;
  };
  let missing: Vec<String> = {
    let flocks = local.flocks.peek();
    let loaded = local.pigeons.peek();
    flocks
      .get(flock_id)
      .map(|f| {
        f.pigeon_ids
          .iter()
          .filter(|id| !loaded.contains_key(*id))
          .cloned()
          .collect()
      })
      .unwrap_or_default()
  };
  if !missing.is_empty() {
    pigeons::list(&missing).await;
  }
}

/// Names from the dashboard's own cache; a pigeon or flock it could not load exports unnamed.
fn names(scope: &GraphScope, local: &LocalSession) -> (HashMap<String, String>, String, String) {
  let pigeons = local.pigeons.peek();
  let flocks = local.flocks.peek();
  match scope {
    GraphScope::Pigeon(pigeon_id) => {
      let pigeon = pigeons.get(pigeon_id);
      let name = pigeon.and_then(|p| p.name.clone()).unwrap_or_default();
      let flock_name = pigeon
        .and_then(|p| flocks.get(&p.flock_id))
        .map(|f| f.name.clone())
        .unwrap_or_default();
      let mut pigeon_names = HashMap::new();
      if !name.is_empty() {
        pigeon_names.insert(pigeon_id.clone(), name.clone());
      }
      (pigeon_names, flock_name, name)
    }
    GraphScope::Flock(flock_id) => {
      let pigeon_names = pigeons
        .values()
        .filter(|p| p.flock_id == *flock_id)
        .filter_map(|p| Some((p.id.clone(), p.name.clone()?)))
        .collect();
      let flock_name = flocks
        .get(flock_id)
        .map(|f| f.name.clone())
        .unwrap_or_default();
      (pigeon_names, flock_name.clone(), flock_name)
    }
  }
}

/// Opened from a telemetry section's header; rendered only while open, so each opening starts
/// from the section's current graphs.
#[component]
pub fn TelemetryExport(
  id: &'static str,
  scope: GraphScope,
  /// The section's graphs, which set the default range and keys.
  graphs: Vec<GraphDef>,
  /// Keys the section knows were reported, offered alongside the graphs' own.
  keys: Vec<String>,
  /// A forwarding pigeon's endpoint: its history is not stored here, which an empty export says.
  forwarding_to: Option<String>,
  on_close: EventHandler<()>,
) -> Element {
  let local = use_context::<LocalSession>();
  let mut range = use_signal(|| default_range(&graphs));
  let mut picked = use_signal(|| default_keys(&graphs));
  let mut all_keys = use_signal(|| picked.peek().is_empty());
  let mut phase = use_signal(|| Phase::Idle);
  let progress = use_signal(|| 0usize);

  let choices = key_choices(&keys, &graphs);
  let busy = matches!(*phase.read(), Phase::Running | Phase::Confirm { .. });
  let can_start = !busy && (all_keys() || !picked.read().is_empty());

  let download = {
    let scope = scope.clone();
    move |_| {
      let now = OffsetDateTime::now_utc();
      let until = now.replace_nanosecond(0).unwrap_or(now);
      let request = Request {
        scope: scope.clone(),
        keys: (!all_keys()).then(|| picked.read().clone()),
        since: until - time::Duration::seconds(range().seconds()),
        until,
        pigeon_names: HashMap::new(),
        flock_name: String::new(),
        scope_name: String::new(),
      };
      start(request, local, phase, progress);
    }
  };

  let status = match &*phase.read() {
    Phase::Idle => rsx! {},
    Phase::Running => rsx! {
      span { class: "loading loading-spinner loading-xs me-2" }
      "Fetching… {progress} points"
    },
    Phase::Confirm {
      walked, left_out, ..
    } => {
      let rows = walked.rows;
      let from = walked.oldest.map(export::rfc3339_utc).unwrap_or_default();
      let to = walked.newest.map(export::rfc3339_utc).unwrap_or_default();
      let cut = export::rfc3339_utc(*left_out);
      rsx! {
        "This range holds more than {rows} points. The file will hold the newest {rows}, from {from} to {to}; points at or before {cut} are left out."
      }
    }
    Phase::Saved { rows, file, older } => {
      let older = older.as_ref().map(|o| export::rfc3339_utc(o.until));
      rsx! {
        "Saved {rows} points to "
        span { class: "font-mono break-all", "{file}" }
        "."
        if let Some(cut) = older {
          " Points at or before {cut} are in the older part."
        }
      }
    }
    Phase::Empty => match forwarding_to.as_ref() {
      Some(url) => rsx! {
        "No points in this range; nothing was saved. This pigeon's telemetry is forwarded to "
        span { class: "font-mono break-all", "{url}" }
        " instead of being stored here."
      },
      None => rsx! { "No points in this range; nothing was saved." },
    },
    Phase::Stalled(Some(at)) => {
      let at = export::rfc3339_utc(*at);
      rsx! {
        "More than {TELEMETRY_HISTORY_MAX_POINTS} points share one second ({at}), so the export cannot page past it. Pick fewer keys. Nothing was saved."
      }
    }
    Phase::Stalled(None) => {
      rsx! { "The server cut the range short but sent no points. Nothing was saved." }
    }
    Phase::Failed(rows) => {
      rsx! { "The export stopped after {rows} points because a request failed. Nothing was saved." }
    }
    Phase::NoHeader => {
      rsx! { "The server did not say whether the range is complete, so nothing was saved." }
    }
    Phase::SaveFailed => rsx! { "The browser did not save the file." },
  };

  let confirm_rows = match &*phase.read() {
    Phase::Confirm { walked, .. } => Some(walked.rows),
    _ => None,
  };
  let has_older = matches!(&*phase.read(), Phase::Saved { older: Some(_), .. });

  rsx! {
    div {
      id,
      class: "border border-base-content/10 rounded-box p-4 flex flex-col gap-4 md:mx-4",
      div { class: "flex items-center justify-between gap-2",
        h3 { class: "font-semibold text-lg", "Export CSV" }
        button {
          class: "btn btn-sm btn-circle btn-ghost",
          r#type: "button",
          "aria-label": "Close export",
          onclick: move |_| on_close.call(()),
          "✕"
        }
      }

      label { class: "flex flex-col gap-1 sm:w-56",
        span { class: "text-xs font-semibold", "Time range" }
        select {
          class: "select select-bordered select-sm w-full",
          disabled: busy,
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

      fieldset { class: "flex flex-col gap-2 min-w-0",
        legend { class: "text-xs font-semibold mb-1", "Keys" }
        label { class: "flex items-center gap-2 text-sm cursor-pointer w-fit",
          input {
            r#type: "checkbox",
            class: "checkbox checkbox-sm",
            disabled: busy,
            checked: all_keys(),
            onchange: move |evt: Event<FormData>| all_keys.set(evt.checked()),
          }
          "All keys"
        }
        if !choices.is_empty() {
          div { class: "flex flex-wrap gap-x-4 gap-y-1 max-h-48 overflow-y-auto",
            for k in choices {
              label { class: "flex items-center gap-2 text-sm cursor-pointer min-w-0",
                input {
                  r#type: "checkbox",
                  class: "checkbox checkbox-sm",
                  disabled: busy || all_keys(),
                  checked: !all_keys() && picked.read().contains(&k),
                  onchange: {
                      let k = k.clone();
                      move |evt: Event<FormData>| {
                          let mut keys = picked.write();
                          if evt.checked() {
                              if !keys.contains(&k) {
                                  keys.push(k.clone());
                              }
                          } else {
                              keys.retain(|existing| existing != &k);
                          }
                      }
                  },
                }
                span { class: "font-mono text-xs break-all", "{k}" }
              }
            }
          }
        }
      }

      div { class: "flex flex-wrap items-center gap-2",
        button {
          class: "btn btn-primary btn-sm",
          r#type: "button",
          disabled: !can_start,
          onclick: download,
          "Download CSV"
        }
        if let Some(rows) = confirm_rows {
          button {
            class: "btn btn-secondary btn-sm",
            r#type: "button",
            onclick: move |_| {
                if let Phase::Confirm { walked, left_out, request } = phase.replace(Phase::Idle) {
                    let since = walked
                        .oldest
                        .map(|t| t.replace_nanosecond(0).unwrap_or(t))
                        .unwrap_or(request.since);
                    let older = Request {
                        until: left_out,
                        ..request.clone()
                    };
                    phase.set(save(walked, &request, since, Some(older)));
                }
            },
            "Download these {rows}"
          }
          button {
            class: "btn btn-ghost btn-sm",
            r#type: "button",
            onclick: move |_| phase.set(Phase::Idle),
            "Cancel"
          }
        }
        if has_older {
          button {
            class: "btn btn-secondary btn-sm",
            r#type: "button",
            onclick: move |_| {
                let older = match &*phase.peek() {
                    Phase::Saved { older: Some(older), .. } => Some(older.clone()),
                    _ => None,
                };
                if let Some(older) = older {
                    start(older, local, phase, progress);
                }
            },
            "Export the older part"
          }
        }
      }

      p { class: "text-sm text-base-content/80", role: "status", "aria-live": "polite", {status} }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{default_keys, default_range, key_choices};
  use crate::components::GraphDef;
  use crate::components::graph_widget::TimeRange;

  fn graph(keys: &[&str], range: TimeRange) -> GraphDef {
    GraphDef {
      id: "g".to_string(),
      title: "g".to_string(),
      keys: keys.iter().map(|k| k.to_string()).collect(),
      range,
      kind: Default::default(),
    }
  }

  #[test]
  fn the_range_on_screen_is_the_widest_graph_else_a_day() {
    assert_eq!(default_range(&[]), TimeRange::Last24h);
    let graphs = [
      graph(&["a"], TimeRange::Last1h),
      graph(&["b"], TimeRange::Last7d),
      graph(&["c"], TimeRange::Last6h),
    ];
    assert_eq!(default_range(&graphs), TimeRange::Last7d);
  }

  #[test]
  fn the_keys_on_screen_are_the_graphs_keys_else_all() {
    assert!(default_keys(&[]).is_empty());
    let graphs = [
      graph(&["uptime_s", "reading"], TimeRange::Last1h),
      graph(&["reading"], TimeRange::Last1h),
    ];
    assert_eq!(default_keys(&graphs), vec!["reading", "uptime_s"]);
  }

  #[test]
  fn choices_merge_reported_and_graphed_keys_that_can_be_filtered() {
    let reported = [
      "rsrp_dbm".to_string(),
      "a,b".to_string(),
      " pad".to_string(),
    ];
    let graphs = [graph(&["uptime_s", "rsrp_dbm"], TimeRange::Last1h)];
    assert_eq!(
      key_choices(&reported, &graphs),
      vec!["rsrp_dbm", "uptime_s"]
    );
  }
}
