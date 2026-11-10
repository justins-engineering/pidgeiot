//! "Add to dashboard" on a saved graph: copies the graph onto a custom dashboard as a time series
//! widget. A copy, not a move, so the graph stays on the page people already use.

use crate::Route;
use crate::api::dashboards::{self, DashboardRead, SaveOutcome};
use crate::components::GraphDef;
use crate::helpers::dashboard_layout::{fresh_widget_id, widget_from_graph};
use crate::helpers::graph_store::GraphScope;
use capsules::{
  Dashboard, DashboardCreateRequest, DashboardLayout, DashboardSummary, DashboardUpdateRequest,
  WidgetScope,
};
use dioxus::prelude::*;
use uuid::Uuid;

fn scope_of(source: &GraphScope) -> WidgetScope {
  match source {
    GraphScope::Pigeon(id) => WidgetScope::Pigeon(id.clone()),
    GraphScope::Flock(id) => WidgetScope::Flock(*id),
  }
}

/// `layout` with the graph appended under a fresh widget id.
fn with_graph(mut layout: DashboardLayout, def: &GraphDef, source: &GraphScope) -> DashboardLayout {
  let id = fresh_widget_id(&layout);
  layout
    .widgets
    .push(widget_from_graph(def, scope_of(source), id));
  layout
}

/// Appends the graph to an existing dashboard. A save that lost to another one is retried once
/// on top of what is now stored, so the other edit is kept rather than overwritten.
async fn append(
  dashboard_id: Uuid,
  def: &GraphDef,
  source: &GraphScope,
) -> Result<Dashboard, String> {
  let DashboardRead::Found(mut current) = dashboards::get(dashboard_id).await else {
    return Err("That dashboard could not be read.".to_string());
  };
  for _ in 0..2 {
    let request = DashboardUpdateRequest {
      name: current.name.clone(),
      layout: with_graph(current.layout.clone(), def, source),
      revision: current.revision,
    };
    match dashboards::save(dashboard_id, &request).await {
      SaveOutcome::Saved(saved) => return Ok(saved),
      SaveOutcome::Conflict(theirs) => current = theirs,
      SaveOutcome::Failed(message) => return Err(message),
    }
  }
  Err("That dashboard kept changing; try again.".to_string())
}

#[component]
pub fn AddToDashboard(def: GraphDef, source: GraphScope) -> Element {
  let mut open = use_signal(|| false);
  let mut choices = use_signal(|| None::<Vec<DashboardSummary>>);
  let mut done = use_signal(|| None::<Dashboard>);
  let mut error = use_signal(|| None::<String>);
  let mut busy = use_signal(|| false);

  let toggle = move |_| async move {
    if open() {
      open.set(false);
      return;
    }
    open.set(true);
    done.set(None);
    error.set(None);
    choices.set(None);
    match dashboards::list().await {
      Some(list) => choices.set(Some(list.into_iter().filter(|d| d.can_edit).collect())),
      None => error.set(Some("Your dashboards could not be loaded.".to_string())),
    }
  };

  rsx! {
    div { class: "relative",
      button {
        class: "btn btn-ghost btn-sm",
        r#type: "button",
        onclick: toggle,
        "Add to dashboard"
      }
      if open() {
        div { class: "absolute right-0 z-10 mt-1 w-64 max-w-[80vw] card bg-base-200 border border-base-300 shadow-xl p-3 flex flex-col gap-2 text-sm",
          if let Some(saved) = done() {
            p { "Added to {saved.name}." }
            Link {
              class: "link link-primary",
              to: Route::DashboardView {
                  dashboard_id: saved.id,
              },
              "Open it"
            }
          } else {
            match choices() {
              None if error().is_none() => rsx! {
                span { class: "loading loading-spinner loading-xs" }
              },
              None => rsx! {},
              Some(list) => rsx! {
                for summary in list {
                  button {
                    key: "{summary.id}",
                    class: "btn btn-sm btn-ghost justify-start font-normal",
                    disabled: busy(),
                    onclick: {
                        let def = def.clone();
                        let source = source.clone();
                        move |_| {
                            let def = def.clone();
                            let source = source.clone();
                            async move {
                                busy.set(true);
                                match append(summary.id, &def, &source).await {
                                    Ok(saved) => done.set(Some(saved)),
                                    Err(message) => error.set(Some(message)),
                                }
                                busy.set(false);
                            }
                        }
                    },
                    "{summary.name}"
                  }
                }
                button {
                  class: "btn btn-sm btn-outline",
                  disabled: busy(),
                  onclick: {
                      let def = def.clone();
                      let source = source.clone();
                      move |_| {
                          let def = def.clone();
                          let source = source.clone();
                          async move {
                              busy.set(true);
                              let request = DashboardCreateRequest {
                                  name: if def.title.trim().is_empty() {
                                      "Saved graph".to_string()
                                  } else {
                                      def.title.clone()
                                  },
                                  org_id: None,
                                  layout: Some(with_graph(DashboardLayout::default(), &def, &source)),
                              };
                              match dashboards::create(&request).await {
                                  Ok(saved) => done.set(Some(saved)),
                                  Err(message) => error.set(Some(message)),
                              }
                              busy.set(false);
                          }
                      }
                  },
                  "New personal dashboard"
                }
              },
            }
          }
          if let Some(message) = error() {
            p { class: "text-error text-xs", "{message}" }
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::components::{ChartKind, TimeRange};

  #[test]
  fn a_graph_lands_after_what_is_there_under_a_fresh_id() {
    let def = GraphDef {
      id: "g".into(),
      title: "Lux".into(),
      keys: vec!["lux".into()],
      range: TimeRange::Last24h,
      kind: ChartKind::Line,
    };
    let flock = Uuid::nil();
    let once = with_graph(DashboardLayout::default(), &def, &GraphScope::Flock(flock));
    let twice = with_graph(once, &def, &GraphScope::Flock(flock));
    let ids: Vec<&str> = twice.widgets.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["w1", "w2"]);
    assert!(capsules::validate_layout(&twice).is_ok());
  }
}
