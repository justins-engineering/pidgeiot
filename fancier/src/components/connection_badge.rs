use crate::helpers::connection_state::{
  ConnectionState, ConnectionStateStyle, format_last_seen, format_last_seen_within,
};
use dioxus::prelude::*;
use time::OffsetDateTime;

/// Colored state badge + human "last seen" caption. Purely
/// presentational -- callers do the classification (see
/// `helpers::connection_state`) since the signals available differ
/// between the pigeon detail page (telemetry + shadow + logs) and the
/// flock pigeon-list (telemetry only, see views/pigeons.rs). A caller that
/// only looked `window_hours` back passes that, so an empty window is not
/// captioned "Never seen".
#[component]
pub fn ConnectionBadge(
  state: ConnectionState,
  last_seen: Option<OffsetDateTime>,
  #[props(default)] window_hours: Option<i64>,
) -> Element {
  let now = OffsetDateTime::now_utc();
  let caption = match window_hours {
    Some(hours) => format_last_seen_within(last_seen, hours, now),
    None => format_last_seen(last_seen, now),
  };
  rsx! {
    div { class: "inline-flex items-center gap-2",
      div { class: "badge {state.badge_class()} gap-1.5",
        span { class: "{state.status_class()}" }
        "{state.label()}"
      }
      span { class: "text-xs text-base-content/60", "{caption}" }
    }
  }
}
