//! What each custom dashboard widget shows. Every widget reads its own data with the viewer's
//! session, so a widget over a flock the viewer cannot see answers "unavailable" and nothing
//! else. A failed read is never drawn as mock data: on a shared operations page a made-up line
//! is worse than a blank tile.

use crate::api::{alerts, dashboards, pigeons, telemetry};
use crate::components::graph_widget::{
  ThresholdAlert, series_from_flock_history, series_from_history, thresholds_from_alerts,
};
use crate::components::{ChartReference, ChartSeries, ConnectionBadge, TelemetryChart};
use crate::helpers::connection_state;
use crate::helpers::dashboard_layout::{chart_kind, percent_of_range, time_range};
use crate::helpers::{is_page_hidden, sleep_ms};
use crate::{LocalSession, Route};
use capsules::connection_state::format_last_seen;
use capsules::{
  AlertStatus, FlockShadowVersions, PigeonLatestTelemetry, TelemetryLatest, WidgetBody,
  WidgetChart, WidgetRange, WidgetScope,
};
use dioxus::prelude::*;
use std::collections::HashMap;
use std::future::Future;
use time::OffsetDateTime;
use uuid::Uuid;

/// A widget's read: still loading, failed (or not permitted), or in hand.
#[derive(Clone, PartialEq)]
enum Read<T> {
  Loading,
  Unavailable,
  Ready(T),
}

/// Polls `fetch` now and then every `every_secs` (the fallback cadence when `None`), skipping
/// while the tab is hidden.
fn use_poll<T, F, Fut>(every_secs: Option<i64>, fetch: F) -> Signal<Read<T>>
where
  T: 'static,
  F: Fn() -> Fut + 'static,
  Fut: Future<Output = Option<T>> + 'static,
{
  let mut read = use_signal(|| Read::Loading);
  let fetch = std::rc::Rc::new(fetch);
  use_future(move || {
    let fetch = fetch.clone();
    async move {
      let poll_ms = connection_state::poll_interval_ms(every_secs);
      loop {
        if !is_page_hidden() {
          match fetch().await {
            Some(value) => read.set(Read::Ready(value)),
            None => read.set(Read::Unavailable),
          }
        }
        sleep_ms(poll_ms).await;
      }
    }
  });
  read
}

/// One pigeon's display name from the shared cache, else its short id.
fn pigeon_label(cache: &HashMap<String, capsules::Pigeon>, id: &str) -> String {
  cache
    .get(id)
    .and_then(|p| p.name.clone())
    .filter(|n| !n.trim().is_empty())
    .unwrap_or_else(|| id.chars().take(8).collect::<String>() + "…")
}

#[component]
fn Unavailable(what: String) -> Element {
  rsx! {
    p { class: "text-sm text-base-content/60 py-6 text-center",
      "Unavailable: {what} could not be read. You may not have access to it, or it could not be reached."
    }
  }
}

#[component]
fn Loading() -> Element {
  rsx! {
    div { class: "flex justify-center py-6",
      span { class: "loading loading-spinner loading-sm text-primary" }
    }
  }
}

/// The body of one widget, remounted whenever what it reads changes: each widget's poll keeps the
/// props it mounted with, so an edited target must start a new poll rather than reuse the old one.
#[component]
pub fn WidgetContent(body: WidgetBody) -> Element {
  let reads = serde_json::to_string(&body).unwrap_or_default();
  rsx! {
    for body in std::iter::once(body) {
      WidgetBodyView { key: "{reads}", body }
    }
  }
}

#[component]
fn WidgetBodyView(body: WidgetBody) -> Element {
  match body {
    WidgetBody::TimeSeries {
      scope,
      keys,
      range,
      chart,
    } => rsx! {
      TimeSeriesWidget { scope, keys, range, chart }
    },
    WidgetBody::Latest { pigeon, key, unit } => rsx! {
      LatestWidget { pigeon, metric: key, unit }
    },
    WidgetBody::Threshold {
      pigeon,
      key,
      unit,
      min,
      max,
    } => rsx! {
      ThresholdWidget {
        pigeon,
        metric: key,
        unit,
        min,
        max,
      }
    },
    WidgetBody::StatusTable { flock, keys } => rsx! {
      StatusTableWidget { flock, keys }
    },
    WidgetBody::ShadowKey { pigeon, key } => rsx! {
      ShadowKeyWidget { pigeon, config_key: key }
    },
    WidgetBody::Alerts { flock } => rsx! {
      AlertsWidget { flock }
    },
    WidgetBody::Text { text } => rsx! {
      p { class: "whitespace-pre-wrap break-words text-sm leading-relaxed", "{text}" }
    },
    WidgetBody::Unknown => rsx! {
      p { class: "text-sm text-base-content/60 py-6 text-center",
        "This widget was added by a newer version of the dashboard. Reload the page to see it."
      }
    },
  }
}

/// Every threshold alert that applies to one pigeon: its own, and its flock's, with the flock's
/// state read for this pigeon alone. An alert list that fails to load draws no thresholds rather
/// than failing the widget.
async fn pigeon_thresholds(pigeon_id: &str) -> Vec<ThresholdAlert> {
  let mut definitions = alerts::list_pigeon(pigeon_id).await.unwrap_or_default();
  let mut states = alerts::state_pigeon(pigeon_id).await.unwrap_or_default();
  let local = consume_context::<LocalSession>();
  let mut flock = local.pigeons.read().get(pigeon_id).map(|p| p.flock_id);
  if flock.is_none() && pigeons::get(pigeon_id).await.is_some() {
    flock = local.pigeons.read().get(pigeon_id).map(|p| p.flock_id);
  }
  if let Some(flock) = flock {
    definitions.extend(alerts::list_flock(flock).await.unwrap_or_default());
    states.extend(
      alerts::state_flock(flock)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.pigeon_id == pigeon_id),
    );
  }
  thresholds_from_alerts(&definitions, &states)
}

#[derive(Clone, PartialEq)]
enum Series {
  Live(Vec<ChartSeries>, Vec<ThresholdAlert>),
  Empty,
}

async fn fetch_series(scope: &WidgetScope, keys: &[String], range: WidgetRange) -> Option<Series> {
  let until = OffsetDateTime::now_utc();
  let since = until - time::Duration::seconds(time_range(range).seconds());
  match scope {
    WidgetScope::Pigeon(pigeon_id) => {
      let buckets = telemetry::get_history_buckets(pigeon_id, since, until).await?;
      if buckets.is_empty() {
        return Some(Series::Empty);
      }
      Some(Series::Live(
        series_from_history(keys, &buckets),
        pigeon_thresholds(pigeon_id).await,
      ))
    }
    WidgetScope::Flock(flock_id) => {
      let buckets = telemetry::get_flock_history_buckets(flock_id, since, until).await?;
      if buckets.is_empty() {
        return Some(Series::Empty);
      }
      // Names for the legend: load the flock's pigeons once if the cache lacks any.
      let local = consume_context::<LocalSession>();
      let ids: Vec<String> = local
        .flocks
        .read()
        .get(flock_id)
        .map(|f| f.pigeon_ids.clone())
        .unwrap_or_default();
      let uncached = ids.iter().any(|id| !local.pigeons.read().contains_key(id));
      if uncached && !ids.is_empty() {
        pigeons::list(&ids).await;
      }
      let cache = local.pigeons.read();
      Some(Series::Live(
        series_from_flock_history(keys, &buckets, |id| pigeon_label(&cache, id)),
        Vec::new(),
      ))
    }
  }
}

/// How often a time series rereads its history. A week or a month is drawn in buckets half an
/// hour to two hours wide, so rereading it at the 30 s fallback mostly repeats the same answer.
fn series_poll_secs(range: WidgetRange) -> Option<i64> {
  match range {
    WidgetRange::Last7d | WidgetRange::Last30d => Some(300),
    _ => None,
  }
}

#[component]
fn TimeSeriesWidget(
  scope: WidgetScope,
  keys: Vec<String>,
  range: WidgetRange,
  chart: WidgetChart,
) -> Element {
  let local = use_context::<LocalSession>();
  let read = {
    let scope = scope.clone();
    let keys = keys.clone();
    use_poll(series_poll_secs(range), move || {
      let scope = scope.clone();
      let keys = keys.clone();
      async move { fetch_series(&scope, &keys, range).await }
    })
  };
  let forwarding = match &scope {
    WidgetScope::Pigeon(id) => local
      .pigeons
      .read()
      .get(id)
      .is_some_and(|p| p.telemetry_endpoint.is_some()),
    WidgetScope::Flock(_) => false,
  };

  match read() {
    Read::Loading => rsx! {
      Loading {}
    },
    Read::Unavailable => rsx! {
      Unavailable { what: "this history".to_string() }
    },
    Read::Ready(Series::Empty) if forwarding => rsx! {
      p { class: "text-sm text-base-content/60 py-6 text-center",
        "No history to chart: this pigeon forwards its telemetry to its own endpoint instead of storing it here."
      }
    },
    Read::Ready(Series::Empty) => rsx! {
      TelemetryChart { series: Vec::new() }
    },
    Read::Ready(Series::Live(series, thresholds)) => {
      let references: Vec<ChartReference> = thresholds
        .iter()
        .filter(|t| keys.contains(&t.key))
        .map(|t| ChartReference {
          value: t.value,
          label: t.label.clone(),
          firing: t.firing,
        })
        .collect();
      rsx! {
        TelemetryChart { series, kind: chart_kind(chart), references }
      }
    }
  }
}

fn reading_of(latest: &[TelemetryLatest], key: &str) -> Option<TelemetryLatest> {
  latest.iter().find(|t| t.key == key).cloned()
}

#[component]
fn LatestWidget(pigeon: String, metric: String, unit: String) -> Element {
  let read = {
    let pigeon = pigeon.clone();
    use_poll(None, move || {
      let pigeon = pigeon.clone();
      async move { telemetry::get_latest(&pigeon).await }
    })
  };
  match read() {
    Read::Loading => rsx! {
      Loading {}
    },
    Read::Unavailable => rsx! {
      Unavailable { what: "this pigeon".to_string() }
    },
    Read::Ready(latest) => match reading_of(&latest, &metric) {
      None => rsx! {
        p { class: "text-sm text-base-content/60 py-6 text-center", "No reading for {metric} yet." }
      },
      Some(reading) => {
        let age = format_last_seen(Some(reading.reported_at), OffsetDateTime::now_utc());
        rsx! {
          div { class: "flex flex-col gap-1 py-2",
            p { class: "text-3xl font-bold break-all",
              "{reading.value}"
              if !unit.is_empty() {
                span { class: "text-lg font-normal text-base-content/60 ml-1", "{unit}" }
              }
            }
            p { class: "text-xs text-base-content/60", "Reported {age}" }
          }
        }
      }
    },
  }
}

#[component]
fn ThresholdWidget(pigeon: String, metric: String, unit: String, min: f64, max: f64) -> Element {
  let read = {
    let pigeon = pigeon.clone();
    use_poll(None, move || {
      let pigeon = pigeon.clone();
      async move {
        let latest = telemetry::get_latest(&pigeon).await?;
        Some((latest, pigeon_thresholds(&pigeon).await))
      }
    })
  };
  match read() {
    Read::Loading => rsx! {
      Loading {}
    },
    Read::Unavailable => rsx! {
      Unavailable { what: "this pigeon".to_string() }
    },
    Read::Ready((latest, thresholds)) => {
      let watching: Vec<ThresholdAlert> =
        thresholds.into_iter().filter(|t| t.key == metric).collect();
      let firing = watching.iter().any(|t| t.firing);
      let Some(reading) = reading_of(&latest, &metric) else {
        return rsx! {
          p { class: "text-sm text-base-content/60 py-6 text-center", "No reading for {metric} yet." }
        };
      };
      let Ok(value) = reading.value.trim().parse::<f64>() else {
        return rsx! {
          p { class: "text-sm text-base-content/60 py-6 text-center",
            "{metric} reads \"{reading.value}\", which is not a number."
          }
        };
      };
      let percent = percent_of_range(value, min, max);
      let (bar, state) = if firing {
        ("progress progress-error", "Alert firing")
      } else if watching.is_empty() {
        (
          "progress progress-primary",
          "No threshold alert on this key",
        )
      } else {
        ("progress progress-success", "No alert firing")
      };
      let age = format_last_seen(Some(reading.reported_at), OffsetDateTime::now_utc());
      rsx! {
        div { class: "flex flex-col gap-2 py-1",
          div { class: "flex items-baseline justify-between gap-2",
            span { class: "text-2xl font-bold", "{reading.value}{unit}" }
            span { class: if firing { "badge badge-error badge-sm" } else { "badge badge-ghost badge-sm" },
              "{state}"
            }
          }
          progress {
            class: "{bar} w-full",
            value: "{percent}",
            max: "100",
            "aria-label": "{metric} between {min} and {max}",
          }
          div { class: "flex justify-between text-xs text-base-content/60",
            span { "{min}{unit}" }
            span { "Reported {age}" }
            span { "{max}{unit}" }
          }
          for t in watching.iter() {
            p { class: "text-xs text-base-content/60", "{t.label}" }
          }
        }
      }
    }
  }
}

/// What a status table reads per poll.
#[derive(Clone, PartialEq)]
struct StatusData {
  missing: Vec<String>,
  telemetry: HashMap<String, Vec<TelemetryLatest>>,
  shadows: Option<FlockShadowVersions>,
}

#[component]
fn StatusTableWidget(flock: Uuid, keys: Vec<String>) -> Element {
  let local = use_context::<LocalSession>();
  let Some(ids) = local
    .flocks
    .read()
    .get(&flock)
    .map(|f| f.pigeon_ids.clone())
  else {
    return rsx! {
      Unavailable { what: "this flock".to_string() }
    };
  };
  // Remounted when the flock's pigeons arrive or change, for the same reason as `WidgetContent`.
  let members = ids.join(",");
  rsx! {
    for ids in std::iter::once(ids) {
      StatusTableRows { key: "{members}", flock, keys: keys.clone(), ids }
    }
  }
}

#[component]
fn StatusTableRows(flock: Uuid, keys: Vec<String>, ids: Vec<String>) -> Element {
  let local = use_context::<LocalSession>();
  let read = {
    let keys = keys.clone();
    let ids = ids.clone();
    use_poll(None, move || {
      let keys = keys.clone();
      let ids = ids.clone();
      async move {
        let missing = if ids.is_empty() {
          Vec::new()
        } else {
          pigeons::list(&ids).await?
        };
        let telemetry: HashMap<String, Vec<TelemetryLatest>> = if keys.is_empty() || ids.is_empty()
        {
          HashMap::new()
        } else {
          dashboards::latest_telemetry(&ids, &keys)
            .await?
            .into_iter()
            .map(
              |PigeonLatestTelemetry {
                 pigeon_id,
                 telemetry,
               }| (pigeon_id, telemetry),
            )
            .collect()
        };
        let shadows = dashboards::shadow_versions(flock, &[]).await;
        Some(StatusData {
          missing,
          telemetry,
          shadows,
        })
      }
    })
  };

  match read() {
    Read::Loading => rsx! {
      Loading {}
    },
    Read::Unavailable => rsx! {
      Unavailable { what: "this flock".to_string() }
    },
    Read::Ready(data) => {
      if ids.is_empty() {
        return rsx! {
          p { class: "text-sm text-base-content/60 py-6 text-center", "This flock has no pigeons yet." }
        };
      }
      let cache = local.pigeons.read();
      let now = OffsetDateTime::now_utc();
      let behind: HashMap<&str, bool> = data
        .shadows
        .as_ref()
        .map(|s| {
          s.pigeons
            .iter()
            .map(|p| (p.pigeon_id.as_str(), p.behind()))
            .collect()
        })
        .unwrap_or_default();
      let rows: Vec<&String> = ids.iter().filter(|id| !data.missing.contains(id)).collect();
      rsx! {
        div { class: "overflow-x-auto",
          table { class: "table table-sm",
            thead {
              tr {
                th { "Pigeon" }
                th { "Connection" }
                for key in keys.iter() {
                  th { class: "font-mono", "{key}" }
                }
                th { "Config" }
              }
            }
            tbody {
              for id in rows {
                tr { key: "{id}",
                  td {
                    Link {
                      class: "link link-hover font-medium",
                      to: Route::PigeonView {
                          flock_id: flock,
                          pigeon_id: id.clone(),
                      },
                      "{pigeon_label(&cache, id)}"
                    }
                  }
                  td {
                    match cache.get(id.as_str()) {
                      Some(pigeon) => rsx! {
                        ConnectionBadge {
                          state: connection_state::contact_state(pigeon, now),
                          last_seen: pigeon.last_seen_at,
                        }
                      },
                      None => rsx! {
                        span { class: "text-base-content/50", "—" }
                      },
                    }
                  }
                  for key in keys.iter() {
                    td { class: "font-mono text-xs",
                      {
                          data.telemetry
                              .get(id.as_str())
                              .and_then(|t| reading_of(t, key))
                              .map(|r| r.value)
                              .unwrap_or_else(|| "—".to_string())
                      }
                    }
                  }
                  td {
                    match behind.get(id.as_str()) {
                      Some(true) => rsx! {
                        span { class: "badge badge-warning badge-sm", "Behind" }
                      },
                      Some(false) => rsx! {
                        span { class: "badge badge-success badge-sm", "Applied" }
                      },
                      None => rsx! {
                        span { class: "text-base-content/50", "—" }
                      },
                    }
                  }
                }
              }
            }
          }
        }
        if !data.missing.is_empty() {
          p { class: "text-xs text-warning mt-2",
            "{data.missing.len()} pigeon(s) in this flock could not be read."
          }
        }
        if data.shadows.is_none() {
          p { class: "text-xs text-base-content/60 mt-2",
            "Config state is unavailable right now."
          }
        }
      }
    }
  }
}

/// A top-level config key's value as display text, `None` when the config does not hold it.
fn config_value(config: &capsules::JsonString, key: &str) -> Option<String> {
  let value: serde_json::Value = serde_json::from_str(&config.to_string()).ok()?;
  let v = value.get(key)?;
  Some(match v {
    serde_json::Value::String(s) => s.clone(),
    other => other.to_string(),
  })
}

#[component]
fn ShadowKeyWidget(pigeon: String, config_key: String) -> Element {
  let read = {
    let pigeon = pigeon.clone();
    use_poll(None, move || {
      let pigeon = pigeon.clone();
      async move { pigeons::get_detail(&pigeon).await.map(|d| d.shadow) }
    })
  };
  match read() {
    Read::Loading => rsx! {
      Loading {}
    },
    Read::Unavailable => rsx! {
      Unavailable { what: "this pigeon's config".to_string() }
    },
    Read::Ready(shadow) => {
      let target =
        config_value(&shadow.target_config, &config_key).unwrap_or_else(|| "not set".to_string());
      let current =
        config_value(&shadow.current_config, &config_key).unwrap_or_else(|| "not set".to_string());
      let (from, to) = (shadow.current_version, shadow.target_version);
      let behind = shadow.current_version < shadow.target_version;
      rsx! {
        div { class: "flex flex-col gap-2 py-1 text-sm",
          div { class: "flex justify-between gap-2",
            span { class: "text-base-content/60", "Target" }
            span { class: "font-mono break-all text-right", "{target}" }
          }
          div { class: "flex justify-between gap-2",
            span { class: "text-base-content/60", "Reported" }
            span { class: "font-mono break-all text-right", "{current}" }
          }
          div {
            if behind {
              span { class: "badge badge-warning badge-sm", "Behind: version {from} of {to}" }
            } else {
              span { class: "badge badge-success badge-sm", "Applied: version {from}" }
            }
          }
        }
      }
    }
  }
}

#[component]
fn AlertsWidget(flock: Uuid) -> Element {
  let read = use_poll(None, move || async move {
    let definitions = alerts::list_flock(flock).await?;
    let states = alerts::state_flock(flock).await?;
    Some((definitions, states))
  });
  match read() {
    Read::Loading => rsx! {
      Loading {}
    },
    Read::Unavailable => rsx! {
      Unavailable { what: "this flock's alerts".to_string() }
    },
    Read::Ready((definitions, states)) => {
      if definitions.is_empty() {
        return rsx! {
          p { class: "text-sm text-base-content/60 py-6 text-center", "This flock has no alerts." }
        };
      }
      rsx! {
        ul { class: "flex flex-col gap-2",
          for definition in definitions.iter() {
            {
                let firing = states
                    .iter()
                    .filter(|s| {
                        s.alert_definition_id == definition.id && s.status == AlertStatus::Firing
                    })
                    .count();
                rsx! {
                  li { key: "{definition.id}", class: "flex items-center justify-between gap-2 text-sm",
                    span { class: "truncate", "{definition.name}" }
                    if !definition.enabled {
                      span { class: "badge badge-ghost badge-sm shrink-0", "Off" }
                    } else if firing > 0 {
                      span { class: "badge badge-error badge-sm shrink-0", "Firing on {firing}" }
                    } else {
                      span { class: "badge badge-success badge-sm shrink-0", "Clear" }
                    }
                  }
                }
            }
          }
        }
        p { class: "text-xs text-base-content/50 mt-3", "Flock alerts only; a pigeon's own alerts are on its page." }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_config_key_reads_as_text_and_an_absent_one_as_none() {
    let config = capsules::JsonString::new(r#"{"layout":"big","interval":60}"#.into()).unwrap();
    assert_eq!(config_value(&config, "layout").as_deref(), Some("big"));
    assert_eq!(config_value(&config, "interval").as_deref(), Some("60"));
    assert_eq!(config_value(&config, "missing"), None);
  }
}
