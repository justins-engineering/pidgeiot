// Fleet-wide dashboard (replaces the old `/dashboard` -> `Flocks` alias):
// a fleet-at-a-glance view built entirely from data the app already fetches
// elsewhere -- no new backend routes. `Flocks` (the create/search/manage
// list) stays reachable at its own `/flocks` route and via the links this
// page renders; this view is a summary layer on top of it, not a
// replacement.
use crate::components::{ConnectorBadge, Maturity, MaturityBadge};
use crate::helpers::connection_state::{self, ConnectionState};
use crate::{Route, api};
use capsules::{Flock, Pigeon};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdBellOff, LdBird, LdLayoutGrid, LdTriangleAlert, LdWifi,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

/// How far back the fleet-wide "last seen" fetch looks, mirroring the
/// per-flock pigeon list's own `LIST_LOOKBACK_HOURS` (views/pigeons.rs) --
/// same reasoning applies here, just fanned out over every flock instead
/// of one: bounded, cheap, and only needs to say "seen recently" vs. not,
/// not produce an exact age.
const FLEET_LOOKBACK_HOURS: i64 = 6;

/// Cap on how many device cards the fleet grid renders directly. Anything
/// past this is one click away via a flock's own pigeon list (which has no
/// such cap) -- this page's job is triage, not a full inventory table.
const MAX_DEVICE_CARDS: usize = 8;

#[component]
pub fn Dashboard() -> Element {
  let local = use_context::<crate::LocalSession>();
  let flocks = local.flocks;
  let pigeons = local.pigeons;
  let mut last_seen: Signal<HashMap<String, OffsetDateTime>> = use_signal(HashMap::new);
  let mut fleet_data_loaded = use_signal(|| false);

  // Populates the two things this page needs that aren't already fetched
  // by the time an authenticated user lands here: each flock's pigeons
  // (LocalSession.pigeons is otherwise only filled in lazily, per flock,
  // by views/pigeons.rs) and a fleet-wide last-seen map (built the same
  // way the per-flock pigeon list builds its own, just once per flock
  // instead of once total -- there is no combined "all my telemetry"
  // route to call instead). Reads `flocks` before the first `.await` so
  // this reruns if the flocks map changes, same pattern as the per-flock
  // pigeon list's own resource (views/pigeons.rs).
  use_resource(move || async move {
    let flock_snapshot: Vec<(Uuid, Vec<String>)> = flocks
      .read()
      .iter()
      .map(|(id, flock)| (*id, flock.pigeon_ids.clone()))
      .collect();

    if flock_snapshot.is_empty() {
      fleet_data_loaded.set(true);
      return;
    }

    let now = OffsetDateTime::now_utc();
    let since = now - time::Duration::hours(FLEET_LOOKBACK_HOURS);
    let mut merged: HashMap<String, OffsetDateTime> = HashMap::new();

    for (flock_id, pigeon_ids) in flock_snapshot {
      if !pigeon_ids.is_empty() {
        api::pigeons::list(&pigeon_ids).await;
      }
      if let Some(points) = api::telemetry::get_flock_history(&flock_id, since, now).await {
        for (id, seen) in connection_state::latest_seen_by_pigeon(&points) {
          merged
            .entry(id)
            .and_modify(|t| {
              if seen > *t {
                *t = seen;
              }
            })
            .or_insert(seen);
        }
      }
    }

    last_seen.set(merged);
    fleet_data_loaded.set(true);
  });

  let flock_map = flocks.read();
  let total_flocks = flock_map.len();

  if total_flocks == 0 {
    return rsx! {
      if fleet_data_loaded() {
        NoFlocksState {}
      } else {
        div { class: "loading loading-spinner text-primary m-10" }
      }
    };
  }

  // LocalSession.pigeons is a shared, additive cache -- nothing ever prunes
  // it (see views/pigeons.rs's own comment on this) -- so scope to pigeons
  // whose flock is one this account currently owns rather than trusting
  // the cache's full contents as "the fleet."
  let scoped_pigeons: Vec<Pigeon> = pigeons
    .read()
    .values()
    .filter(|pigeon| flock_map.contains_key(&pigeon.flock_id))
    .cloned()
    .collect();
  let total_pigeons = scoped_pigeons.len();

  let now = OffsetDateTime::now_utc();
  let seen = last_seen.read();
  let mut online = 0usize;
  let mut stale = 0usize;
  let mut offline = 0usize;
  let mut unknown = 0usize;
  let mut per_flock: HashMap<Uuid, FlockConnStats> = HashMap::new();

  let mut classified: Vec<(Pigeon, ConnectionState, Option<OffsetDateTime>)> = scoped_pigeons
    .iter()
    .map(|pigeon| {
      let last = seen.get(&pigeon.id).copied();
      let state = connection_state::classify(last, None, now);
      let bucket = per_flock.entry(pigeon.flock_id).or_default();
      match state {
        ConnectionState::Online => {
          online += 1;
          bucket.online += 1;
        }
        ConnectionState::Stale => {
          stale += 1;
          bucket.stale += 1;
        }
        ConnectionState::Offline => {
          offline += 1;
          bucket.offline += 1;
        }
        ConnectionState::Unknown => {
          unknown += 1;
          bucket.unknown += 1;
        }
      }
      (pigeon.clone(), state, last)
    })
    .collect();
  drop(seen);

  // Worst-first: a fleet dashboard's job is triage, so problem devices
  // float to the top of the (capped) grid rather than sorting
  // alphabetically or by recency.
  classified.sort_by_key(|(_, state, _)| match state {
    ConnectionState::Offline => 0,
    ConnectionState::Stale => 1,
    ConnectionState::Unknown => 2,
    ConnectionState::Online => 3,
  });

  let needs_attention = stale + offline;
  let pct = |n: usize| -> f64 {
    if total_pigeons == 0 {
      0.0
    } else {
      (n as f64 / total_pigeons as f64) * 100.0
    }
  };

  let mut flock_list: Vec<Flock> = flock_map.values().cloned().collect();
  flock_list.sort_by_key(|flock| flock.name.to_lowercase());
  drop(flock_map);

  rsx! {
    section { id: "dashboard",
      div { class: "my-1 max-w-7xl mx-auto w-full",

        header { class: "flex flex-col md:flex-row items-start md:items-center justify-between gap-4 mb-8",
          div {
            h1 { class: "text-2xl font-bold", "Fleet Overview" }
            p { class: "text-base-content/60 text-sm mt-1",
              "{total_pigeons} pigeons across {total_flocks} flocks"
            }
          }
          Link {
            to: Route::Flocks {},
            class: "btn btn-outline btn-primary sm:px-6",
            "Manage Flocks"
          }
        }

        // Stat row
        div { class: "stats shadow-sm bg-base-100 border border-base-content/10 w-full grid grid-cols-2 lg:grid-cols-4 mb-8",
          div { class: "stat",
            div { class: "stat-figure text-secondary",
              Icon { width: 28, height: 28, icon: LdLayoutGrid, title: "Flocks" }
            }
            div { class: "stat-title", "Flocks" }
            div { class: "stat-value text-secondary", "{total_flocks}" }
            div { class: "stat-desc", "Groups you own" }
          }
          div { class: "stat",
            div { class: "stat-figure text-primary",
              Icon { width: 28, height: 28, icon: LdBird, title: "Pigeons" }
            }
            div { class: "stat-title", "Pigeons" }
            div { class: "stat-value text-primary", "{total_pigeons}" }
            div { class: "stat-desc", "Registered devices" }
          }
          div { class: "stat",
            div { class: "stat-figure text-success",
              Icon { width: 26, height: 26, icon: LdWifi, title: "Online" }
            }
            div { class: "stat-title", "Online" }
            div { class: "stat-value text-success", "{online}" }
            div { class: "stat-desc", "Seen within cadence" }
          }
          div { class: "stat",
            div { class: "stat-figure text-warning",
              Icon { width: 26, height: 26, icon: LdTriangleAlert, title: "Needs attention" }
            }
            div { class: "stat-title", "Needs Attention" }
            div { class: "stat-value text-warning", "{needs_attention}" }
            div { class: "stat-desc", "Stale or offline" }
          }
        }

        div { class: "grid grid-cols-1 lg:grid-cols-3 gap-6",
          // Main column
          div { class: "lg:col-span-2 flex flex-col gap-6",

            // Fleet health bar
            div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
              h2 { class: "text-lg font-bold mb-4", "Fleet Health" }
              if total_pigeons == 0 {
                p { class: "text-sm text-base-content/60",
                  "No pigeons registered yet — fleet health will appear here once you register one."
                }
              } else {
                div { class: "w-full h-3 rounded-full overflow-hidden flex bg-base-300",
                  div {
                    class: "h-full bg-success",
                    style: "width: {pct(online)}%",
                  }
                  div {
                    class: "h-full bg-warning",
                    style: "width: {pct(stale)}%",
                  }
                  div {
                    class: "h-full bg-error",
                    style: "width: {pct(offline)}%",
                  }
                  div {
                    class: "h-full bg-base-content/20",
                    style: "width: {pct(unknown)}%",
                  }
                }
                div { class: "flex flex-wrap gap-x-6 gap-y-2 mt-4 text-sm",
                  div { class: "flex items-center gap-2",
                    span { class: "status status-success" }
                    "Online "
                    span { class: "font-semibold", "{online}" }
                  }
                  div { class: "flex items-center gap-2",
                    span { class: "status status-warning" }
                    "Stale "
                    span { class: "font-semibold", "{stale}" }
                  }
                  div { class: "flex items-center gap-2",
                    span { class: "status status-error" }
                    "Offline "
                    span { class: "font-semibold", "{offline}" }
                  }
                  div { class: "flex items-center gap-2",
                    span { class: "status" }
                    "Unknown "
                    span { class: "font-semibold", "{unknown}" }
                  }
                }
              }
            }

            // Devices needing a look
            div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
              div { class: "flex items-center justify-between mb-4",
                h2 { class: "text-lg font-bold", "Devices" }
                if total_pigeons > 0 {
                  span { class: "text-xs text-base-content/50",
                    "Sorted by status · showing {classified.len().min(MAX_DEVICE_CARDS)} of {total_pigeons}"
                  }
                }
              }
              if total_pigeons == 0 {
                if fleet_data_loaded() {
                  div { class: "flex flex-col items-center text-center gap-2 py-10",
                    p { class: "text-base-content/60 max-w-sm text-sm",
                      "No pigeons registered yet. Open a flock to register your first device."
                    }
                  }
                } else {
                  div { class: "flex justify-center py-10",
                    span { class: "loading loading-spinner text-primary" }
                  }
                }
              } else {
                div { class: "grid grid-cols-1 sm:grid-cols-2 gap-3",
                  for (pigeon , state , seen) in classified.iter().take(MAX_DEVICE_CARDS) {
                    DeviceCard {
                      key: "{pigeon.id}",
                      pigeon: pigeon.clone(),
                      flock_name: flocks
                          .read()
                          .get(&pigeon.flock_id)
                          .map(|f| f.name.clone())
                          .unwrap_or_else(|| "Unknown flock".to_string()),
                      state: *state,
                      last_seen: *seen,
                    }
                  }
                }
                div { class: "flex justify-end mt-4",
                  Link {
                    to: Route::Flocks {},
                    class: "btn btn-ghost btn-sm text-base-content/60",
                    "View all pigeons by flock →"
                  }
                }
              }
            }
          }

          // Sidebar column
          div { class: "flex flex-col gap-6",

            // Flocks quick nav
            div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
              div { class: "flex items-center justify-between mb-4",
                h2 { class: "text-lg font-bold", "Flocks" }
                Link {
                  to: Route::Flocks {},
                  class: "link link-hover text-xs text-base-content/60",
                  "View all →"
                }
              }
              div { class: "flex flex-col gap-3",
                for flock in flock_list.iter() {
                  FlockNavItem {
                    key: "{flock.id}",
                    flock: flock.clone(),
                    stats: per_flock.get(&flock.id).copied().unwrap_or_default(),
                  }
                }
              }
            }

            // Alerts placeholder -- the alerts backend (task #32) has no
            // data or dashboard-facing API yet, so this is an honest
            // empty state rather than any fabricated alert.
            div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
              div { class: "flex items-center justify-between mb-3",
                h2 { class: "text-lg font-bold", "Alerts" }
                MaturityBadge { maturity: Maturity::Planned }
              }
              div { class: "flex flex-col items-center text-center gap-2 py-6",
                Icon {
                  width: 32,
                  height: 32,
                  icon: LdBellOff,
                  class: "text-base-content/30",
                  title: "Alerts coming soon",
                }
                p { class: "text-sm text-base-content/60 max-w-[22ch]",
                  "Alerting on telemetry & device state is coming soon."
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
fn NoFlocksState() -> Element {
  rsx! {
    div { class: "flex flex-col items-center text-center gap-3 bg-base-100 border border-base-content/10 rounded-box p-12 max-w-xl mx-auto mt-10",
      Icon {
        width: 40,
        height: 40,
        icon: LdLayoutGrid,
        class: "text-base-content/30",
      }
      h2 { class: "text-lg font-semibold", "Welcome to PidgeIoT" }
      p { class: "text-base-content/60 max-w-sm",
        "Create your first flock to start registering pigeons and tracking your fleet."
      }
      Link { to: Route::Flocks {}, class: "btn btn-primary mt-2", "Create a Flock" }
    }
  }
}

/// Per-flock rollup of the same classification the main fleet grid computes,
/// used only by the sidebar's `FlockNavItem` -- kept separate from the
/// fleet-wide `online`/`stale`/`offline`/`unknown` counters above since a
/// glance at one flock's health shouldn't require re-deriving it from the
/// full pigeon list.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct FlockConnStats {
  online: usize,
  stale: usize,
  offline: usize,
  unknown: usize,
}

/// Picks the single most-informative status to show next to a flock in the
/// sidebar: worst state present wins (offline, then stale, then unknown),
/// falling back to an online count only once nothing worse is present.
/// Returns `None` when nothing has been classified for this flock yet (no
/// pigeons, or the fleet-wide fetch hasn't resolved) so the caller can omit
/// the indicator entirely rather than assert a state with no data behind
/// it.
fn flock_status_summary(stats: FlockConnStats) -> Option<(&'static str, String)> {
  if stats.offline > 0 {
    Some(("status-error", format!("{} offline", stats.offline)))
  } else if stats.stale > 0 {
    Some(("status-warning", format!("{} stale", stats.stale)))
  } else if stats.online > 0 {
    Some(("status-success", format!("{} online", stats.online)))
  } else if stats.unknown > 0 {
    Some(("status", format!("{} unknown", stats.unknown)))
  } else {
    None
  }
}

#[component]
fn FlockNavItem(flock: Flock, stats: FlockConnStats) -> Element {
  let summary = flock_status_summary(stats);

  rsx! {
    Link {
      to: Route::Pigeons { flock_id: flock.id },
      class: "flex items-center justify-between rounded-box border border-base-content/10 p-3 hover:border-primary/40 hover:bg-base-200/40 transition-colors",
      div {
        div { class: "font-semibold text-secondary text-sm", "{flock.name}" }
        div { class: "text-xs text-base-content/50 mt-0.5",
          "{flock.pigeon_ids.len()} pigeons"
        }
      }
      if let Some((dot_class, label)) = summary {
        div { class: "flex items-center gap-1 shrink-0",
          span { class: "status {dot_class}" }
          span { class: "text-xs text-base-content/60", "{label}" }
        }
      }
    }
  }
}

#[component]
fn DeviceCard(
  pigeon: Pigeon,
  flock_name: String,
  state: ConnectionState,
  last_seen: Option<OffsetDateTime>,
) -> Element {
  let now = OffsetDateTime::now_utc();
  let card_theme = match state {
    ConnectionState::Offline => "border-error/30 bg-error/5",
    ConnectionState::Stale => "border-warning/30 bg-warning/5",
    ConnectionState::Unknown => "border-base-content/10 bg-base-200/40",
    ConnectionState::Online => "border-base-content/10",
  };
  let flock_id = pigeon.flock_id;
  let pigeon_id = pigeon.id.clone();
  let display_name = pigeon.name.clone().unwrap_or_else(|| pigeon.id.clone());

  rsx! {
    div { class: "border {card_theme} rounded-box p-4 flex flex-col gap-2",
      div { class: "flex items-center justify-between gap-2",
        span { class: "font-semibold text-primary truncate", "{display_name}" }
        div { class: "badge {state.badge_class()} gap-1.5 shrink-0",
          span { class: "{state.status_class()}" }
          "{state.label()}"
        }
      }
      div { class: "flex items-center gap-2 text-xs text-base-content/60",
        ConnectorBadge { connector: pigeon.connector.clone() }
        span { class: "truncate", "{flock_name}" }
      }
      div { class: "flex items-center justify-between text-xs",
        span { class: "text-base-content/50",
          "{connection_state::format_last_seen(last_seen, now)}"
        }
        Link {
          to: Route::PigeonView { flock_id, pigeon_id: pigeon_id.clone() },
          class: "link link-hover text-base-content/60",
          "View →"
        }
      }
    }
  }
}

#[cfg(test)]
mod flock_status_summary_tests {
  use super::{FlockConnStats, flock_status_summary};

  #[test]
  fn offline_wins_over_everything() {
    let stats = FlockConnStats {
      online: 3,
      stale: 1,
      offline: 1,
      unknown: 1,
    };
    let (class, label) = flock_status_summary(stats).unwrap();
    assert_eq!(class, "status-error");
    assert_eq!(label, "1 offline");
  }

  #[test]
  fn stale_wins_over_online_and_unknown() {
    let stats = FlockConnStats {
      online: 2,
      stale: 2,
      offline: 0,
      unknown: 1,
    };
    let (class, label) = flock_status_summary(stats).unwrap();
    assert_eq!(class, "status-warning");
    assert_eq!(label, "2 stale");
  }

  #[test]
  fn online_shown_when_nothing_worse() {
    let stats = FlockConnStats {
      online: 4,
      stale: 0,
      offline: 0,
      unknown: 0,
    };
    let (class, label) = flock_status_summary(stats).unwrap();
    assert_eq!(class, "status-success");
    assert_eq!(label, "4 online");
  }

  #[test]
  fn unknown_only_when_nothing_else_present() {
    let stats = FlockConnStats {
      online: 0,
      stale: 0,
      offline: 0,
      unknown: 2,
    };
    let (class, label) = flock_status_summary(stats).unwrap();
    assert_eq!(class, "status");
    assert_eq!(label, "2 unknown");
  }

  #[test]
  fn no_data_is_none() {
    assert!(flock_status_summary(FlockConnStats::default()).is_none());
  }
}
