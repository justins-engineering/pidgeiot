// Connection-state indicator. This module is a thin re-export of the
// classify/threshold logic in `capsules::connection_state` plus the
// DaisyUI-specific presentation (`ConnectionStateStyle` below) that has no
// business living in a Worker/Dioxus-agnostic crate.
//
// The logic itself lives in `capsules` because `dovecote`'s scheduled
// missing-heartbeat/device-state alert evaluator
// (`dovecote/src/helpers/alerts.rs::evaluate_scheduled_alerts`) needs the
// exact same "online/stale/offline" thresholding this dashboard badge uses
// -- one implementation shared by both sides instead of two forks of the
// same math drifting apart. See `capsules::connection_state`'s own doc
// comment for the full rationale.
//
// "Last seen" is still the newest of whichever signals a given call site
// fetches (see `views::pigeon::PigeonView` for the full three-signal
// version, and `views::pigeons`/`views::dashboard` for the telemetry-only
// version -- neither does a per-pigeon shadow/log fan-out).
pub use capsules::connection_state::{
  ConnectionState, classify, format_last_seen, has_never_reported, latest_of,
  latest_seen_by_pigeon, telemetry_interval_secs,
};

/// Floor and fallback for `poll_interval_ms` -- fancier-only (dovecote never
/// polls itself), so unlike `classify`'s thresholds these don't need to live
/// in `capsules`.
const POLL_FLOOR_SECS: i64 = 10;
const POLL_FALLBACK_SECS: i64 = 30;

/// Auto-refresh cadence for a live-polling dashboard widget (`GraphCard`,
/// `LogViewer`), self-calibrated from a pigeon's own `telemetry_interval`
/// the same way `classify`'s online/stale thresholds are: a pigeon that
/// reports every 10s gets refreshed close to that cadence, one that reports
/// every 10 minutes doesn't get polled needlessly often. Floored so a
/// misconfigured tiny interval can't hammer the Durable Object; falls back
/// to a fixed default for a pigeon with no interval configured yet, mirroring
/// how `classify` falls back to `DEFAULT_ONLINE_SECS`/`DEFAULT_STALE_SECS`.
///
/// Read once when a polling loop starts (see `GraphCard`/`LogViewer`), not
/// re-evaluated live if the pigeon's `telemetry_interval` changes mid-visit
/// -- same one-shot-per-mount tradeoff `views::demo`'s fixed `REFRESH_MS`
/// makes, just computed instead of constant.
pub fn poll_interval_ms(interval_secs: Option<i64>) -> i32 {
  let secs = interval_secs
    .filter(|s| *s > 0)
    .map(|s| s.max(POLL_FLOOR_SECS))
    .unwrap_or(POLL_FALLBACK_SECS);
  (secs * 1000) as i32
}

/// DaisyUI badge/status styling for `ConnectionState` -- kept here (not in
/// `capsules`) since it's Dioxus-dashboard-specific presentation, not
/// shared logic. An extension trait rather than an inherent impl: since
/// `ConnectionState` is defined in `capsules`, Rust's orphan rules don't
/// let `fancier` add inherent methods to it directly (only a
/// locally-defined trait implemented for a foreign type is allowed) --
/// callers just need this trait in scope alongside `ConnectionState`
/// (see `components::connection_badge`, `views::dashboard`).
///
/// `Unknown` deliberately does NOT use `badge-neutral`/`status-neutral`:
/// this theme's `--color-neutral` (assets/tailwind.css) is literal white in
/// light mode and literal black in dark mode, so a neutral badge renders as
/// invisible white-on-white (or barely-there black-on-black) text.
/// `badge-ghost` and the unmodified `.status` class both derive from
/// `--color-base-content` instead, which is always legible against the page.
pub trait ConnectionStateStyle {
  /// DaisyUI badge color token -- conventional online=success/stale=
  /// warning/offline=error semantics.
  fn badge_class(&self) -> &'static str;
  /// Full class list for the DaisyUI `status` indicator dot (includes the
  /// base `status` class, since `Unknown` has no color modifier to pair
  /// it with).
  fn status_class(&self) -> &'static str;
  fn label(&self) -> &'static str;
}

impl ConnectionStateStyle for ConnectionState {
  fn badge_class(&self) -> &'static str {
    match self {
      ConnectionState::Online => "badge-success",
      ConnectionState::Stale => "badge-warning",
      ConnectionState::Offline => "badge-error",
      ConnectionState::Unknown => "badge-ghost",
    }
  }

  fn status_class(&self) -> &'static str {
    match self {
      ConnectionState::Online => "status status-success",
      ConnectionState::Stale => "status status-warning",
      ConnectionState::Offline => "status status-error",
      ConnectionState::Unknown => "status",
    }
  }

  fn label(&self) -> &'static str {
    match self {
      ConnectionState::Online => "Online",
      ConnectionState::Stale => "Stale",
      ConnectionState::Offline => "Offline",
      ConnectionState::Unknown => "Unknown",
    }
  }
}
