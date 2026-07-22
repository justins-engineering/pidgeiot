// User-defined alerts UI (task #32) -- the dashboard-facing last mile over
// dovecote's already-deployed alert CRUD routes (docs/api.md's "Alert
// Routes" section, dovecote/src/helpers/alerts.rs). Two entry points,
// `PigeonAlerts`/`FlockAlerts`, mirror `components::graph_widget`'s
// `PigeonGraphs`/`FlockGraphs` split exactly: same self-contained
// fetch-then-render shape, same reason for staying two components instead
// of one ("Pigeon scope: ... Flock scope: ..." — see that module's own doc
// comment) rather than a single component branching internally.
//
// Named `alerts_panel.rs`/`AlertsPanel`, not `alert.rs`/`Alert` -- this
// crate already has an unrelated toast `components::Alert`
// (`AlertVariant` in `models/`), and `docs/design/alerts-triggers.md` §0
// calls out avoiding that name collision explicitly for every domain type
// introduced by this feature.
use crate::LocalSession;
use crate::api;
use capsules::{
  AlertChannel, AlertCondition, AlertDefinition, AlertDefinitionCreateRequest,
  AlertDefinitionUpdateRequest, AlertScope, AlertSeverity, Comparator, ConnectionStateKind,
  TelemetryHistoryPoint, TelemetryLatest,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdBellRing, LdPencil, LdPlus, LdTrash, LdX};
use uuid::Uuid;

/// How far back `FlockAlerts` looks to build its telemetry-key picker --
/// matches `graph_widget::TimeRange::Last24h`, the same default range
/// `FlockGraphs` uses for its own best-effort key list (no flock-level
/// "latest keys" route exists, per that module's own comment).
const FLOCK_KEY_LOOKBACK_HOURS: i64 = 24;

const COMPARATORS: [Comparator; 5] = [
  Comparator::Gt,
  Comparator::Gte,
  Comparator::Lt,
  Comparator::Lte,
  Comparator::Eq,
];

fn comparator_label(c: Comparator) -> &'static str {
  match c {
    Comparator::Gt => ">",
    Comparator::Gte => ">=",
    Comparator::Lt => "<",
    Comparator::Lte => "<=",
    Comparator::Eq => "=",
  }
}

fn comparator_from_label(s: &str) -> Option<Comparator> {
  COMPARATORS.into_iter().find(|c| comparator_label(*c) == s)
}

fn state_label(s: ConnectionStateKind) -> &'static str {
  match s {
    ConnectionStateKind::Offline => "Offline",
    ConnectionStateKind::Stale => "Stale",
  }
}

fn state_from_label(s: &str) -> Option<ConnectionStateKind> {
  match s {
    "Offline" => Some(ConnectionStateKind::Offline),
    "Stale" => Some(ConnectionStateKind::Stale),
    _ => None,
  }
}

fn severity_badge_class(s: AlertSeverity) -> &'static str {
  match s {
    AlertSeverity::Warning => "badge-warning",
    AlertSeverity::Critical => "badge-error",
  }
}

fn severity_label(s: AlertSeverity) -> &'static str {
  match s {
    AlertSeverity::Warning => "Warning",
    AlertSeverity::Critical => "Critical",
  }
}

fn severity_from_label(s: &str) -> AlertSeverity {
  match s {
    "Critical" => AlertSeverity::Critical,
    _ => AlertSeverity::Warning,
  }
}

/// Compact human rendering of a duration in seconds -- whole hours or
/// minutes print without a remainder, anything else falls back to seconds.
/// Pure/unit-tested since it's exactly the kind of small formatting logic
/// this codebase already carves out as a standalone function (see
/// `format_bytes` in `firmware_modal.rs`).
fn duration_label(secs: i64) -> String {
  if secs <= 0 {
    "0s".to_string()
  } else if secs % 3600 == 0 {
    format!("{}h", secs / 3600)
  } else if secs % 60 == 0 {
    format!("{}m", secs / 60)
  } else {
    format!("{secs}s")
  }
}

/// One-line summary of an alert's condition for the list table -- the four
/// variants `capsules::AlertCondition` has today. All four are evaluated
/// by the backend: `Threshold`/`RateOfChange` at every telemetry ingest
/// (`check_telemetry_alerts`, task #39 added the latter),
/// `DeviceState`/`MissingReport` by the Cron-Trigger-driven scheduled sweep
/// (`evaluate_scheduled_alerts`, task #38) -- see
/// `dovecote/src/helpers/alerts.rs`.
fn condition_summary(condition: &AlertCondition) -> String {
  match condition {
    AlertCondition::Threshold {
      key,
      comparator,
      value,
    } => format!("{key} {} {value}", comparator_label(*comparator)),
    AlertCondition::DeviceState {
      state,
      min_duration_secs,
    } => match min_duration_secs {
      Some(secs) if *secs > 0 => {
        format!(
          "device state = {} for \u{2265} {}",
          state_label(*state),
          duration_label(*secs)
        )
      }
      _ => format!("device state = {}", state_label(*state)),
    },
    AlertCondition::MissingReport { max_silence_secs } => {
      format!(
        "no telemetry for \u{2265} {}",
        duration_label(*max_silence_secs)
      )
    }
    AlertCondition::RateOfChange {
      key,
      max_delta,
      window_secs,
    } => match window_secs {
      Some(secs) => format!(
        "{key} changes by > {max_delta} within {}",
        duration_label(*secs)
      ),
      None => format!("{key} changes by > {max_delta}"),
    },
  }
}

/// Telemetry keys eligible for a Threshold alert (task #32 point 4): a
/// non-numeric-valued key can't be compared against a numeric threshold,
/// same rule the telemetry graph section models via
/// `TelemetryHistoryPoint::value_num` (see CLAUDE.md's telemetry-forwarding
/// note -- non-numeric values are stored but excluded from anything
/// numeric). `TelemetryLatest` only carries the raw string `value` (no
/// pre-parsed `value_num`), so this parses it the same way dovecote's own
/// `write_telemetry_history` decides numeric-ness server-side, rather than
/// guessing some other rule client-side.
fn numeric_keys_from_latest(latest: &[TelemetryLatest]) -> Vec<String> {
  let mut keys: Vec<String> = latest
    .iter()
    .filter(|l| l.value.trim().parse::<f64>().is_ok())
    .map(|l| l.key.clone())
    .collect();
  keys.sort();
  keys.dedup();
  keys
}

/// Same rule, sourced from history points (`FlockAlerts`'s picker) --
/// `value_num` is already parsed server-side here, so no re-parsing needed.
fn numeric_keys_from_history(points: &[TelemetryHistoryPoint]) -> Vec<String> {
  let mut keys: Vec<String> = points
    .iter()
    .filter(|p| p.value_num.is_some())
    .map(|p| p.key.clone())
    .collect();
  keys.sort();
  keys.dedup();
  keys
}

#[component]
pub fn PigeonAlerts(pigeon_id: String) -> Element {
  let mut available_keys: Signal<Vec<String>> = use_signal(Vec::new);

  {
    let pigeon_id = pigeon_id.clone();
    use_resource(move || {
      let pigeon_id = pigeon_id.clone();
      async move {
        api::alerts::list_pigeon(&pigeon_id).await;
        if let Some(latest) = api::telemetry::get_latest(&pigeon_id).await {
          available_keys.set(numeric_keys_from_latest(&latest));
        }
      }
    });
  }

  rsx! {
    AlertsSection {
      scope: AlertScope::Pigeon(pigeon_id),
      available_keys: available_keys(),
    }
  }
}

#[component]
pub fn FlockAlerts(flock_id: Uuid) -> Element {
  let mut available_keys: Signal<Vec<String>> = use_signal(Vec::new);

  use_resource(move || async move {
    api::alerts::list_flock(flock_id).await;
    let until = time::OffsetDateTime::now_utc();
    let since = until - time::Duration::hours(FLOCK_KEY_LOOKBACK_HOURS);
    if let Some(points) = api::telemetry::get_flock_history(&flock_id, since, until).await {
      available_keys.set(numeric_keys_from_history(&points));
    }
  });

  rsx! {
    AlertsSection {
      scope: AlertScope::Flock(flock_id),
      available_keys: available_keys(),
    }
  }
}

#[component]
fn AlertsSection(scope: AlertScope, available_keys: Vec<String>) -> Element {
  let local = use_context::<LocalSession>();
  let mut show_add = use_signal(|| false);
  let mut editing: Signal<Option<AlertDefinition>> = use_signal(|| None);
  let mut deleting: Signal<Option<AlertDefinition>> = use_signal(|| None);

  let scope_for_filter = scope.clone();
  let alerts: Vec<AlertDefinition> = local
    .alerts
    .read()
    .values()
    .filter(|a| a.scope == scope_for_filter)
    .cloned()
    .collect();
  let mut alerts = alerts;
  alerts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        div { class: "flex items-center gap-2",
          Icon { width: 22, height: 22, icon: LdBellRing }
          h2 { class: "text-3xl font-bold", "Alerts" }
        }
        button {
          class: "btn btn-secondary",
          onclick: move |_| show_add.set(true),
          Icon { width: 16, height: 16, icon: LdPlus }
          "Add Alert"
        }
      }

      if alerts.is_empty() {
        p { class: "text-sm text-base-content/50 italic md:px-4",
          "No alerts yet. Add one to get notified on telemetry thresholds or device state."
        }
      } else {
        div { class: "overflow-x-auto rounded-box border border-base-content/10",
          table { class: "table table-zebra w-full",
            thead {
              tr { class: "bg-base-200/50 text-base-content",
                th { "Name" }
                th { "Condition" }
                th { "Severity" }
                th { "Enabled" }
                th { class: "text-right", "Actions" }
              }
            }
            tbody {
              for alert in alerts.iter().cloned() {
                AlertRow {
                  key: "{alert.id}",
                  alert: alert.clone(),
                  on_edit: move |a| editing.set(Some(a)),
                  on_delete: move |a| deleting.set(Some(a)),
                }
              }
            }
          }
        }
      }
    }

    if show_add() {
      AlertFormModal {
        scope: scope.clone(),
        available_keys: available_keys.clone(),
        editing: None,
        on_close: move |_| show_add.set(false),
        on_saved: move |_| show_add.set(false),
      }
    }

    if let Some(alert) = editing() {
      AlertFormModal {
        scope: scope.clone(),
        available_keys: available_keys.clone(),
        editing: Some(alert),
        on_close: move |_| editing.set(None),
        on_saved: move |_| editing.set(None),
      }
    }

    if let Some(alert) = deleting() {
      DeleteAlertModal {
        alert,
        on_close: move |_| deleting.set(None),
        on_deleted: move |_| deleting.set(None),
      }
    }
  }
}

#[component]
fn AlertRow(
  alert: AlertDefinition,
  on_edit: EventHandler<AlertDefinition>,
  on_delete: EventHandler<AlertDefinition>,
) -> Element {
  let mut is_toggling = use_signal(|| false);
  let alert_id = alert.id;
  let enabled = alert.enabled;

  rsx! {
    tr { class: "hover",
      td { class: "font-semibold text-primary", "{alert.name}" }
      td { class: "font-mono text-xs text-base-content/80",
        div { "{condition_summary(&alert.condition)}" }
      }
      td {
        span {
          class: "badge badge-sm {severity_badge_class(alert.severity)}",
          "{alert.severity.as_str()}"
        }
      }
      td {
        input {
          r#type: "checkbox",
          class: "toggle toggle-sm toggle-success",
          checked: enabled,
          disabled: is_toggling(),
          onchange: move |evt: Event<FormData>| {
              let checked = evt.checked();
              async move {
                  is_toggling.set(true);
                  let req = AlertDefinitionUpdateRequest {
                      enabled: Some(checked),
                      ..Default::default()
                  };
                  api::alerts::update(alert_id, &req).await;
                  is_toggling.set(false);
              }
          },
        }
      }
      td { class: "text-right",
        div { class: "flex justify-end gap-1",
          button {
            class: "btn btn-ghost btn-xs",
            title: "Edit",
            onclick: {
                let alert = alert.clone();
                move |_| on_edit.call(alert.clone())
            },
            Icon { width: 14, height: 14, icon: LdPencil }
          }
          button {
            class: "btn btn-ghost btn-xs text-error",
            title: "Delete",
            onclick: {
                let alert = alert.clone();
                move |_| on_delete.call(alert.clone())
            },
            Icon { width: 14, height: 14, icon: LdTrash }
          }
        }
      }
    }
  }
}

#[derive(Clone, Copy, PartialEq)]
enum ConditionKind {
  Threshold,
  DeviceState,
  MissingReport,
  RateOfChange,
}

/// Create/edit form (task #32). Rendered conditionally by `AlertsSection`
/// rather than a native `<dialog>`, per CLAUDE.md's reset-sensitive-modal
/// pattern (`EditShadowModal`/`FirmwareModal`/`DeletePigeonModal`) -- every
/// field here is derived from `editing` at mount time, so opening this for
/// a *different* alert (or for "Add" after editing one) always remounts
/// with fresh state rather than carrying over stale input.
#[component]
fn AlertFormModal(
  scope: AlertScope,
  available_keys: Vec<String>,
  editing: Option<AlertDefinition>,
  on_close: EventHandler<()>,
  on_saved: EventHandler<AlertDefinition>,
) -> Element {
  let is_edit = editing.is_some();
  let editing_id = editing.as_ref().map(|a| a.id);

  let mut name = use_signal(|| editing.as_ref().map(|a| a.name.clone()).unwrap_or_default());

  let mut condition_kind = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::DeviceState { .. }) => ConditionKind::DeviceState,
    Some(AlertCondition::MissingReport { .. }) => ConditionKind::MissingReport,
    Some(AlertCondition::RateOfChange { .. }) => ConditionKind::RateOfChange,
    _ => ConditionKind::Threshold,
  });

  let mut key = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::Threshold { key, .. }) => key.clone(),
    Some(AlertCondition::RateOfChange { key, .. }) => key.clone(),
    _ => available_keys.first().cloned().unwrap_or_default(),
  });
  let mut comparator = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::Threshold { comparator, .. }) => *comparator,
    _ => Comparator::Lt,
  });
  let mut value_input = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::Threshold { value, .. }) => value.to_string(),
    _ => String::new(),
  });

  let mut max_delta_input = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::RateOfChange { max_delta, .. }) => max_delta.to_string(),
    _ => String::new(),
  });
  let mut window_secs_input = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::RateOfChange {
      window_secs: Some(secs),
      ..
    }) => (secs / 60).to_string(),
    _ => String::new(),
  });

  let mut device_state = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::DeviceState { state, .. }) => *state,
    _ => ConnectionStateKind::Offline,
  });
  let mut min_duration_input = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::DeviceState {
      min_duration_secs: Some(secs),
      ..
    }) => (secs / 60).to_string(),
    _ => String::new(),
  });

  let mut max_silence_input = use_signal(|| match editing.as_ref().map(|a| &a.condition) {
    Some(AlertCondition::MissingReport { max_silence_secs }) => (max_silence_secs / 60).to_string(),
    _ => String::new(),
  });

  let mut severity = use_signal(|| editing.as_ref().map(|a| a.severity).unwrap_or_default());
  let mut recipient = use_signal(|| match editing.as_ref().map(|a| &a.channel) {
    Some(AlertChannel::Email { to: Some(addr) }) => addr.clone(),
    _ => String::new(),
  });

  let mut is_saving = use_signal(|| false);
  let mut submit_error = use_signal(|| Option::<String>::None);

  let scope_label = match &scope {
    AlertScope::Pigeon(id) => format!("This pigeon — {id}"),
    AlertScope::Flock(id) => format!("This flock — {id}"),
  };

  let threshold_value_valid = value_input.read().trim().parse::<f64>().is_ok();
  let max_silence_valid = max_silence_input
    .read()
    .trim()
    .parse::<i64>()
    .is_ok_and(|m| m > 0);
  let max_delta_valid = max_delta_input
    .read()
    .trim()
    .parse::<f64>()
    .is_ok_and(|v| v > 0.0);
  let window_secs_valid = window_secs_input.read().trim().is_empty()
    || window_secs_input
      .read()
      .trim()
      .parse::<i64>()
      .is_ok_and(|m| m > 0);
  let can_submit = !name.read().trim().is_empty()
    && match condition_kind() {
      ConditionKind::Threshold => !key.read().trim().is_empty() && threshold_value_valid,
      ConditionKind::DeviceState => true,
      ConditionKind::MissingReport => max_silence_valid,
      ConditionKind::RateOfChange => {
        !key.read().trim().is_empty() && max_delta_valid && window_secs_valid
      }
    };

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "alert_modal_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape && !is_saving() {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-lg",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          disabled: is_saving(),
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 {
          class: "text-lg font-bold",
          id: "alert_modal_title",
          if is_edit { "Edit Alert" } else { "New Alert" }
        }

        form {
          class: "mt-3",
          onsubmit: move |evt: FormEvent| {
              evt.prevent_default();
              let scope = scope.clone();
              async move {
                  if !can_submit || is_saving() {
                      return;
                  }
                  let condition = match condition_kind() {
                      ConditionKind::Threshold => {
                          let Ok(value) = value_input.read().trim().parse::<f64>() else {
                              return;
                          };
                          AlertCondition::Threshold {
                              key: key.read().trim().to_string(),
                              comparator: comparator(),
                              value,
                          }
                      }
                      ConditionKind::DeviceState => {
                          let min_duration_secs = min_duration_input
                              .read()
                              .trim()
                              .parse::<i64>()
                              .ok()
                              .filter(|m| *m > 0)
                              .map(|minutes| minutes * 60);
                          AlertCondition::DeviceState {
                              state: device_state(),
                              min_duration_secs,
                          }
                      }
                      ConditionKind::MissingReport => {
                          let Ok(minutes) = max_silence_input.read().trim().parse::<i64>() else {
                              return;
                          };
                          if minutes <= 0 {
                              return;
                          }
                          AlertCondition::MissingReport {
                              max_silence_secs: minutes * 60,
                          }
                      }
                      ConditionKind::RateOfChange => {
                          let Ok(max_delta) = max_delta_input.read().trim().parse::<f64>() else {
                              return;
                          };
                          if max_delta <= 0.0 {
                              return;
                          }
                          let window_raw = window_secs_input.read().trim().to_string();
                          let window_secs = if window_raw.is_empty() {
                              None
                          } else {
                              let Ok(minutes) = window_raw.parse::<i64>() else {
                                  return;
                              };
                              if minutes <= 0 {
                                  return;
                              }
                              Some(minutes * 60)
                          };
                          AlertCondition::RateOfChange {
                              key: key.read().trim().to_string(),
                              max_delta,
                              window_secs,
                          }
                      }
                  };
                  let recipient_value = recipient.read().trim().to_string();
                  let channel = AlertChannel::Email {
                      to: if recipient_value.is_empty() { None } else { Some(recipient_value) },
                  };

                  is_saving.set(true);
                  submit_error.set(None);

                  let result = if let Some(id) = editing_id {
                      let req = AlertDefinitionUpdateRequest {
                          name: Some(name.read().trim().to_string()),
                          condition: Some(condition),
                          severity: Some(severity()),
                          channel: Some(channel),
                          enabled: None,
                      };
                      api::alerts::update(id, &req).await
                  } else {
                      let req = AlertDefinitionCreateRequest {
                          name: name.read().trim().to_string(),
                          condition,
                          severity: severity(),
                          channel,
                      };
                      match scope {
                          AlertScope::Pigeon(pigeon_id) => {
                              api::alerts::create_pigeon(&pigeon_id, &req).await
                          }
                          AlertScope::Flock(flock_id) => {
                              api::alerts::create_flock(flock_id, &req).await
                          }
                      }
                  };

                  is_saving.set(false);
                  match result {
                      Some(alert) => on_saved.call(alert),
                      None => {
                          submit_error
                              .set(Some("Failed to save alert. Please try again.".to_string()));
                      }
                  }
              }
          },

          fieldset { class: "fieldset flex flex-col gap-4",
            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1", "Name" }
              input {
                class: "input input-bordered w-full text-sm",
                r#type: "text",
                placeholder: "e.g., Low battery",
                disabled: is_saving(),
                value: "{name}",
                oninput: move |e| name.set(e.value()),
              }
            }

            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1", "Scope" }
              div { class: "text-sm bg-base-200 rounded px-3 py-2 font-mono", "{scope_label}" }
            }

            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1", "Condition type" }
              select {
                class: "select select-bordered w-full text-sm",
                disabled: is_saving(),
                value: match condition_kind() {
                    ConditionKind::Threshold => "Threshold",
                    ConditionKind::DeviceState => "DeviceState",
                    ConditionKind::MissingReport => "MissingReport",
                    ConditionKind::RateOfChange => "RateOfChange",
                },
                onchange: move |evt: Event<FormData>| {
                    condition_kind
                        .set(
                            match evt.value().as_str() {
                                "DeviceState" => ConditionKind::DeviceState,
                                "MissingReport" => ConditionKind::MissingReport,
                                "RateOfChange" => ConditionKind::RateOfChange,
                                _ => ConditionKind::Threshold,
                            },
                        );
                },
                option { value: "Threshold", "Threshold (telemetry value)" }
                option { value: "DeviceState", "Device State (offline / stale)" }
                option { value: "MissingReport", "Missing Report (no telemetry)" }
                option { value: "RateOfChange", "Rate of Change (spike detection)" }
              }
            }

            if condition_kind() == ConditionKind::Threshold {
              div { class: "grid grid-cols-3 gap-2",
                div { class: "col-span-3 sm:col-span-1",
                  label { class: "fieldset-legend text-xs font-semibold mb-1", "Telemetry key" }
                  if available_keys.is_empty() {
                    input {
                      class: "input input-bordered input-sm w-full text-sm font-mono",
                      r#type: "text",
                      placeholder: "e.g., battery_mv",
                      disabled: is_saving(),
                      value: "{key}",
                      oninput: move |e| key.set(e.value()),
                    }
                  } else {
                    select {
                      class: "select select-bordered select-sm w-full text-sm",
                      disabled: is_saving(),
                      value: "{key}",
                      onchange: move |evt: Event<FormData>| key.set(evt.value()),
                      for k in available_keys.iter().cloned() {
                        option { value: "{k}", selected: k == key(), "{k}" }
                      }
                    }
                  }
                }
                div { class: "col-span-1",
                  label { class: "fieldset-legend text-xs font-semibold mb-1", "Comparator" }
                  select {
                    class: "select select-bordered select-sm w-full text-sm",
                    disabled: is_saving(),
                    value: comparator_label(comparator()),
                    onchange: move |evt: Event<FormData>| {
                        if let Some(c) = comparator_from_label(&evt.value()) {
                            comparator.set(c);
                        }
                    },
                    for c in COMPARATORS {
                      option {
                        value: comparator_label(c),
                        selected: c == comparator(),
                        "{comparator_label(c)}"
                      }
                    }
                  }
                }
                div { class: "col-span-2 sm:col-span-1",
                  label { class: "fieldset-legend text-xs font-semibold mb-1", "Value" }
                  input {
                    class: "input input-bordered input-sm w-full text-sm",
                    r#type: "number",
                    step: "any",
                    disabled: is_saving(),
                    value: "{value_input}",
                    oninput: move |e| value_input.set(e.value()),
                  }
                }
              }
              p { class: "text-xs text-base-content/60 -mt-2",
                "Only numeric telemetry keys reported by this "
                if matches!(scope, AlertScope::Flock(_)) { "flock" } else { "pigeon" }
                " can be thresholded."
              }
              if !threshold_value_valid && !value_input.read().is_empty() {
                p { class: "text-error text-xs -mt-2", "Value must be a number." }
              }
            } else if condition_kind() == ConditionKind::DeviceState {
              div { class: "grid grid-cols-2 gap-2",
                div {
                  label { class: "fieldset-legend text-xs font-semibold mb-1", "State" }
                  select {
                    class: "select select-bordered select-sm w-full text-sm",
                    disabled: is_saving(),
                    value: state_label(device_state()),
                    onchange: move |evt: Event<FormData>| {
                        if let Some(s) = state_from_label(&evt.value()) {
                            device_state.set(s);
                        }
                    },
                    option { value: "Offline", selected: device_state() == ConnectionStateKind::Offline, "Offline" }
                    option { value: "Stale", selected: device_state() == ConnectionStateKind::Stale, "Stale" }
                  }
                }
                div {
                  label { class: "fieldset-legend text-xs font-semibold mb-1",
                    "Min duration (minutes, optional)"
                  }
                  input {
                    class: "input input-bordered input-sm w-full text-sm",
                    r#type: "number",
                    min: "0",
                    placeholder: "e.g., 30",
                    disabled: is_saving(),
                    value: "{min_duration_input}",
                    oninput: move |e| min_duration_input.set(e.value()),
                  }
                }
              }
              p { class: "text-xs text-base-content/60 -mt-2",
                "Fires when this pigeon is classified in the chosen state for at least the given duration (checked every few minutes, not instantly)."
              }
            } else if condition_kind() == ConditionKind::MissingReport {
              div {
                label { class: "fieldset-legend text-xs font-semibold mb-1", "Max silence (minutes)" }
                input {
                  class: "input input-bordered input-sm w-full text-sm",
                  r#type: "number",
                  min: "1",
                  placeholder: "e.g., 60",
                  disabled: is_saving(),
                  value: "{max_silence_input}",
                  oninput: move |e| max_silence_input.set(e.value()),
                }
              }
              p { class: "text-xs text-base-content/60 -mt-2",
                "Fires when no telemetry (any key) has been reported for this pigeon in at least this many minutes."
              }
              if !max_silence_valid && !max_silence_input.read().is_empty() {
                p { class: "text-error text-xs -mt-2", "Enter a whole number of minutes greater than 0." }
              }
            } else {
              div { class: "grid grid-cols-3 gap-2",
                div { class: "col-span-3 sm:col-span-1",
                  label { class: "fieldset-legend text-xs font-semibold mb-1", "Telemetry key" }
                  if available_keys.is_empty() {
                    input {
                      class: "input input-bordered input-sm w-full text-sm font-mono",
                      r#type: "text",
                      placeholder: "e.g., battery_mv",
                      disabled: is_saving(),
                      value: "{key}",
                      oninput: move |e| key.set(e.value()),
                    }
                  } else {
                    select {
                      class: "select select-bordered select-sm w-full text-sm",
                      disabled: is_saving(),
                      value: "{key}",
                      onchange: move |evt: Event<FormData>| key.set(evt.value()),
                      for k in available_keys.iter().cloned() {
                        option { value: "{k}", selected: k == key(), "{k}" }
                      }
                    }
                  }
                }
                div { class: "col-span-1",
                  label { class: "fieldset-legend text-xs font-semibold mb-1", "Max delta" }
                  input {
                    class: "input input-bordered input-sm w-full text-sm",
                    r#type: "number",
                    step: "any",
                    min: "0",
                    placeholder: "e.g., 500",
                    disabled: is_saving(),
                    value: "{max_delta_input}",
                    oninput: move |e| max_delta_input.set(e.value()),
                  }
                }
                div { class: "col-span-2 sm:col-span-1",
                  label { class: "fieldset-legend text-xs font-semibold mb-1",
                    "Within (minutes, optional)"
                  }
                  input {
                    class: "input input-bordered input-sm w-full text-sm",
                    r#type: "number",
                    min: "1",
                    placeholder: "e.g., 10",
                    disabled: is_saving(),
                    value: "{window_secs_input}",
                    oninput: move |e| window_secs_input.set(e.value()),
                  }
                }
              }
              p { class: "text-xs text-base-content/60 -mt-2",
                "Fires when this "
                if matches!(scope, AlertScope::Flock(_)) { "flock's" } else { "pigeon's" }
                " telemetry key changes by more than the given amount between two reports."
                " Leave \"within\" blank to compare against the previous report regardless of"
                " how long ago it was; set it to ignore a jump after a long silence."
              }
              if !max_delta_valid && !max_delta_input.read().is_empty() {
                p { class: "text-error text-xs -mt-2", "Max delta must be a positive number." }
              }
              if !window_secs_valid {
                p { class: "text-error text-xs -mt-2", "Enter a whole number of minutes greater than 0, or leave blank." }
              }
            }

            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1", "Severity" }
              select {
                class: "select select-bordered w-full text-sm",
                disabled: is_saving(),
                value: severity_label(severity()),
                onchange: move |evt: Event<FormData>| severity.set(severity_from_label(&evt.value())),
                option { value: "Warning", selected: severity() == AlertSeverity::Warning, "Warning" }
                option {
                  value: "Critical",
                  selected: severity() == AlertSeverity::Critical,
                  "Critical"
                }
              }
            }

            div {
              label { class: "fieldset-legend text-xs font-semibold mb-1", "Notify (email)" }
              input {
                class: "input input-bordered w-full text-sm",
                r#type: "email",
                placeholder: "defaults to flock owner's email",
                disabled: is_saving(),
                value: "{recipient}",
                oninput: move |e| recipient.set(e.value()),
              }
              p { class: "text-xs text-base-content/60 mt-1",
                "Leave blank to notify the flock owner's own address."
              }
            }
          }

          if let Some(err) = submit_error.read().as_ref() {
            p { class: "text-error text-xs mt-3", "⚠️ {err}" }
          }

          div { class: "mt-6 flex items-center justify-end gap-3",
            button {
              class: "btn btn-ghost btn-sm sm:btn-md",
              r#type: "button",
              disabled: is_saving(),
              onclick: move |_| on_close.call(()),
              "Cancel"
            }
            button {
              class: "btn btn-primary shadow-md min-w-[120px]",
              r#type: "submit",
              disabled: !can_submit || is_saving(),
              if is_saving() {
                span { class: "loading loading-spinner loading-sm" }
              } else if is_edit {
                "Save Changes"
              } else {
                "Save Alert"
              }
            }
          }
        }
      }
    }
  }
}

/// Plain confirm, not the typed-name-to-confirm pattern `DeletePigeonModal`
/// uses -- per docs/design/alerts-triggers.md §4's own callout, deleting an
/// alert has no data-loss blast radius the way deleting a pigeon does (no
/// device deauthorization, no irreversible token loss), so the extra
/// friction isn't warranted here.
#[component]
fn DeleteAlertModal(
  alert: AlertDefinition,
  on_close: EventHandler<()>,
  on_deleted: EventHandler<()>,
) -> Element {
  let mut is_deleting = use_signal(|| false);
  let mut error_msg = use_signal(|| Option::<String>::None);
  let alert_id = alert.id;
  let name = alert.name.clone();

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "delete_alert_title",
      onkeydown: move |e| {
          if e.key() == Key::Escape && !is_deleting() {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-sm",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          disabled: is_deleting(),
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 { class: "text-lg font-bold text-error", id: "delete_alert_title", "Delete Alert" }
        p { class: "py-4 text-sm text-base-content/80",
          "Delete "
          strong { "\"{name}\"" }
          "? This cannot be undone."
        }
        if let Some(err) = error_msg.read().as_ref() {
          p { class: "text-error text-xs mb-2", "⚠️ {err}" }
        }
        div { class: "modal-action",
          button {
            class: "btn btn-ghost",
            disabled: is_deleting(),
            onclick: move |_| on_close.call(()),
            "Cancel"
          }
          button {
            class: "btn btn-error",
            disabled: is_deleting(),
            onclick: move |_| async move {
                is_deleting.set(true);
                error_msg.set(None);
                if api::alerts::delete(alert_id).await.is_some() {
                    on_deleted.call(());
                } else {
                    is_deleting.set(false);
                    error_msg.set(Some("Failed to delete alert. Please try again.".to_string()));
                }
            },
            if is_deleting() {
              span { class: "loading loading-spinner loading-sm" }
            } else {
              "Delete Alert"
            }
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    comparator_from_label, comparator_label, condition_summary, duration_label,
    numeric_keys_from_history, numeric_keys_from_latest, state_from_label, state_label,
  };
  use capsules::{
    AlertCondition, Comparator, ConnectionStateKind, TelemetryHistoryPoint, TelemetryLatest,
  };
  use time::OffsetDateTime;

  fn latest(key: &str, value: &str) -> TelemetryLatest {
    TelemetryLatest {
      key: key.to_string(),
      value: value.to_string(),
      reported_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  fn history_point(
    pigeon_id: &str,
    key: &str,
    value: &str,
    value_num: Option<f64>,
  ) -> TelemetryHistoryPoint {
    TelemetryHistoryPoint {
      pigeon_id: pigeon_id.to_string(),
      key: key.to_string(),
      value: value.to_string(),
      value_num,
      reported_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  #[test]
  fn comparator_label_roundtrips() {
    for c in [
      Comparator::Gt,
      Comparator::Gte,
      Comparator::Lt,
      Comparator::Lte,
      Comparator::Eq,
    ] {
      assert_eq!(comparator_from_label(comparator_label(c)), Some(c));
    }
  }

  #[test]
  fn comparator_from_label_rejects_garbage() {
    assert_eq!(comparator_from_label("nope"), None);
  }

  #[test]
  fn state_label_roundtrips() {
    for s in [ConnectionStateKind::Offline, ConnectionStateKind::Stale] {
      assert_eq!(state_from_label(state_label(s)), Some(s));
    }
  }

  #[test]
  fn duration_label_prefers_whole_hours() {
    assert_eq!(duration_label(7200), "2h");
  }

  #[test]
  fn duration_label_prefers_whole_minutes_over_seconds() {
    assert_eq!(duration_label(90), "90s");
    assert_eq!(duration_label(120), "2m");
  }

  #[test]
  fn duration_label_zero_or_negative_is_0s() {
    assert_eq!(duration_label(0), "0s");
    assert_eq!(duration_label(-5), "0s");
  }

  #[test]
  fn threshold_condition_summary() {
    let cond = AlertCondition::Threshold {
      key: "battery_mv".to_string(),
      comparator: Comparator::Lt,
      value: 3300.0,
    };
    assert_eq!(condition_summary(&cond), "battery_mv < 3300");
  }

  #[test]
  fn device_state_condition_summary_without_duration() {
    let cond = AlertCondition::DeviceState {
      state: ConnectionStateKind::Offline,
      min_duration_secs: None,
    };
    assert_eq!(condition_summary(&cond), "device state = Offline");
  }

  #[test]
  fn device_state_condition_summary_with_duration() {
    let cond = AlertCondition::DeviceState {
      state: ConnectionStateKind::Stale,
      min_duration_secs: Some(1800),
    };
    assert_eq!(
      condition_summary(&cond),
      "device state = Stale for \u{2265} 30m"
    );
  }

  #[test]
  fn missing_report_condition_summary() {
    let cond = AlertCondition::MissingReport {
      max_silence_secs: 3600,
    };
    assert_eq!(condition_summary(&cond), "no telemetry for \u{2265} 1h");
  }

  #[test]
  fn rate_of_change_condition_summary_without_window() {
    let cond = AlertCondition::RateOfChange {
      key: "battery_mv".to_string(),
      max_delta: 500.0,
      window_secs: None,
    };
    assert_eq!(condition_summary(&cond), "battery_mv changes by > 500");
  }

  #[test]
  fn rate_of_change_condition_summary_with_window() {
    let cond = AlertCondition::RateOfChange {
      key: "rssi_dbm".to_string(),
      max_delta: 10.0,
      window_secs: Some(600),
    };
    assert_eq!(
      condition_summary(&cond),
      "rssi_dbm changes by > 10 within 10m"
    );
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

  #[test]
  fn numeric_keys_from_latest_dedups_and_sorts() {
    let latest = vec![
      latest("uptime_s", "10"),
      latest("battery_mv", "3300"),
      latest("uptime_s", "20"),
    ];
    assert_eq!(
      numeric_keys_from_latest(&latest),
      vec!["battery_mv", "uptime_s"]
    );
  }

  #[test]
  fn numeric_keys_from_history_excludes_non_numeric() {
    let points = vec![
      history_point("p1", "battery_mv", "3300", Some(3300.0)),
      history_point("p1", "fw_version", "1.2.0", None),
    ];
    assert_eq!(numeric_keys_from_history(&points), vec!["battery_mv"]);
  }
}
