use crate::helpers::connection_state::{ConnectionState, ConnectionStateStyle, format_last_seen};
use dioxus::prelude::*;
use time::OffsetDateTime;

/// Colored state badge + human "last seen" caption. Purely
/// presentational -- callers do the classification (see
/// `helpers::connection_state`), since the pigeon detail page folds more
/// evidence into `last_seen` than the flock list holds.
#[component]
pub fn ConnectionBadge(state: ConnectionState, last_seen: Option<OffsetDateTime>) -> Element {
  let caption = format_last_seen(last_seen, OffsetDateTime::now_utc());
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
