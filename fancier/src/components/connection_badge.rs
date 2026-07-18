use crate::helpers::connection_state::{ConnectionState, format_last_seen};
use dioxus::prelude::*;
use time::OffsetDateTime;

/// Colored state badge + human "last seen" caption (task #31). Purely
/// presentational -- callers do the classification (see
/// `helpers::connection_state`) since the signals available differ
/// between the pigeon detail page (telemetry + shadow + logs) and the
/// flock pigeon-list (telemetry only, see views/pigeons.rs).
#[component]
pub fn ConnectionBadge(state: ConnectionState, last_seen: Option<OffsetDateTime>) -> Element {
  let now = OffsetDateTime::now_utc();
  rsx! {
    div { class: "inline-flex items-center gap-2",
      div { class: "badge {state.badge_class()} gap-1.5",
        span { class: "{state.status_class()}" }
        "{state.label()}"
      }
      span { class: "text-xs text-base-content/60", "{format_last_seen(last_seen, now)}" }
    }
  }
}
