// Connection-state indicator (task #31): derived entirely from telemetry,
// shadow, and log data the dashboard already fetches -- no new device
// traffic, no new backend routes. "Last seen" is the newest of whichever
// signals are available for a given call site (see PigeonView for the
// full three-signal version, and the flock pigeon-list for the
// telemetry-only version -- the list intentionally has no per-pigeon
// shadow/log fan-out, see views/pigeons.rs).
//
// Known imprecision, accepted by design: `PigeonShadow.updated_at` bumps
// on ANY write to the shadow row (`set_shadow_updated_at` trigger,
// dovecote/src/objects/pigeons.rs), including a dashboard user pushing a
// new `target_config` -- not just a device reporting `current_config`
// back. A pigeon that's actually offline can therefore appear briefly
// "seen" right after someone edits its config. This is self-correcting
// (the timestamp doesn't keep advancing, so the pigeon falls back to
// stale/offline once the classification window elapses) and considered
// an acceptable trade-off rather than a reason to add a new backend
// signal.
use capsules::{JsonString, TelemetryHistoryPoint};
use std::collections::HashMap;
use time::OffsetDateTime;

/// Fallback thresholds used when a pigeon's shadow has no
/// `telemetry_interval` configured yet (the shadow config schema is
/// `log`/`telemetry_interval`/`reboot`, all optional -- CLAUDE.md).
const DEFAULT_ONLINE_SECS: i64 = 5 * 60;
const DEFAULT_STALE_SECS: i64 = 30 * 60;

/// Multiples of a pigeon's own reporting cadence that count as "online"
/// vs. "stale" before falling into "offline".
const ONLINE_INTERVAL_MULTIPLE: i64 = 2;
const STALE_INTERVAL_MULTIPLE: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
  /// Seen within ~2x its own telemetry_interval (or 5 minutes with no
  /// interval configured).
  Online,
  /// Seen within ~5x its interval (or 30 minutes), but not recently
  /// enough to count as online.
  Stale,
  /// Last seen further back than the stale window -- or a window-bounded
  /// query (the flock list) found nothing recent at all.
  Offline,
  /// No telemetry, shadow report-back, or log chunk has ever been
  /// observed for this pigeon.
  Unknown,
}

impl ConnectionState {
  /// DaisyUI status/badge color token -- same tokens used for the
  /// "Rolling out" badges elsewhere (badge-primary etc.), picked here for
  /// conventional online=success/stale=warning/offline=error semantics.
  /// `Unknown` deliberately does NOT use `badge-neutral`/`status-neutral`:
  /// this theme's `--color-neutral` is literal white in light mode and
  /// literal black in dark mode (assets/tailwind.css), so a neutral badge
  /// renders as invisible white-on-white (or barely-there black-on-black)
  /// text -- confirmed by screenshotting both themes. `badge-ghost` and
  /// the unmodified `.status` class both derive from `--color-base-content`
  /// instead, which is always legible against the page.
  pub fn badge_class(self) -> &'static str {
    match self {
      ConnectionState::Online => "badge-success",
      ConnectionState::Stale => "badge-warning",
      ConnectionState::Offline => "badge-error",
      ConnectionState::Unknown => "badge-ghost",
    }
  }

  /// Full class list for the DaisyUI `status` indicator dot (includes the
  /// base `status` class, since `Unknown` has no color modifier to pair
  /// it with).
  pub fn status_class(self) -> &'static str {
    match self {
      ConnectionState::Online => "status status-success",
      ConnectionState::Stale => "status status-warning",
      ConnectionState::Offline => "status status-error",
      ConnectionState::Unknown => "status",
    }
  }

  pub fn label(self) -> &'static str {
    match self {
      ConnectionState::Online => "Online",
      ConnectionState::Stale => "Stale",
      ConnectionState::Offline => "Offline",
      ConnectionState::Unknown => "Unknown",
    }
  }
}

/// Pulls `telemetry_interval` out of a shadow's `current_config` (what
/// the device is actually running, not `target_config`, which may not be
/// applied yet) -- same extraction shape as
/// `components::firmware_modal::extract_firmware_target`. `None` covers
/// both "not valid JSON" and "no `telemetry_interval` key set".
pub fn telemetry_interval_secs(config: &JsonString) -> Option<i64> {
  let value: serde_json::Value = serde_json::from_str(&config.to_string()).ok()?;
  value.get("telemetry_interval")?.as_i64()
}

/// True when a pigeon's shadow shows no evidence a device has ever
/// reported back -- `current_version` is still the SQLite-default `0`
/// (a real device report bumps it, see dovecote's `report_shadow_device`)
/// and `current_config` is still the empty object it starts as. Used by
/// `classify` (v1, frontend-only heuristic -- see its own doc comment) to
/// stop a shadow write timestamp from reading as a "seen" signal for a
/// pigeon that has never actually connected. A pigeon that's connected at
/// least once and later goes offline keeps its real `current_version`/
/// `current_config`, so this never fires again for it, even offline.
pub fn has_never_reported(current_version: i32, current_config: &JsonString) -> bool {
  if current_version != 0 {
    return false;
  }
  serde_json::from_str::<serde_json::Value>(&current_config.to_string())
    .ok()
    .and_then(|v| v.as_object().map(|obj| obj.is_empty()))
    .unwrap_or(false)
}

/// The newest of however many "last seen" signals are available. `None`
/// entries (a signal that wasn't fetched, or came back empty) are simply
/// ignored; an all-`None` input means "never seen".
pub fn latest_of(times: impl IntoIterator<Item = Option<OffsetDateTime>>) -> Option<OffsetDateTime> {
  times.into_iter().flatten().max()
}

/// Classifies a pigeon's connection state from its most recent "seen"
/// signal and (if known) its own reporting cadence. Clock skew tolerance:
/// a `last_seen` in the future (device clock ahead of the dashboard's) is
/// treated as "now" rather than producing a negative age.
///
/// Deliberately takes one already-merged `last_seen`, not the individual
/// telemetry/shadow/log signals that fed it: callers decide which signals
/// are trustworthy (see `has_never_reported`, used by `PigeonView` to drop
/// a never-confirmed shadow's `updated_at` before merging) and hand this
/// function only the result, so it stays a pure threshold check.
pub fn classify(
  last_seen: Option<OffsetDateTime>,
  interval_secs: Option<i64>,
  now: OffsetDateTime,
) -> ConnectionState {
  let Some(last_seen) = last_seen else {
    return ConnectionState::Unknown;
  };
  let age_secs = (now - last_seen).whole_seconds().max(0);
  let (online_secs, stale_secs) = match interval_secs {
    Some(interval) if interval > 0 => (
      interval * ONLINE_INTERVAL_MULTIPLE,
      interval * STALE_INTERVAL_MULTIPLE,
    ),
    _ => (DEFAULT_ONLINE_SECS, DEFAULT_STALE_SECS),
  };
  if age_secs <= online_secs {
    ConnectionState::Online
  } else if age_secs <= stale_secs {
    ConnectionState::Stale
  } else {
    ConnectionState::Offline
  }
}

/// Human "last seen X ago" string. Same clock-skew handling as
/// `classify`.
pub fn format_last_seen(last_seen: Option<OffsetDateTime>, now: OffsetDateTime) -> String {
  let Some(last_seen) = last_seen else {
    return "Never seen".to_string();
  };
  let age_secs = (now - last_seen).whole_seconds().max(0);
  if age_secs < 60 {
    "just now".to_string()
  } else if age_secs < 3600 {
    format!("{}m ago", age_secs / 60)
  } else if age_secs < 86400 {
    format!("{}h ago", age_secs / 3600)
  } else {
    format!("{}d ago", age_secs / 86400)
  }
}

/// Per-pigeon last-seen map from one flock-scoped telemetry history query
/// (`GET /flocks/:id/telemetry/history`) -- deliberately the only fetch
/// the flock pigeon-list makes for this feature (see views/pigeons.rs):
/// no per-pigeon fan-out, no shadow/log signals, just the newest
/// `reported_at` per `pigeon_id` within whatever window the caller
/// queried.
pub fn latest_seen_by_pigeon(points: &[TelemetryHistoryPoint]) -> HashMap<String, OffsetDateTime> {
  let mut latest: HashMap<String, OffsetDateTime> = HashMap::new();
  for point in points {
    latest
      .entry(point.pigeon_id.clone())
      .and_modify(|t| {
        if point.reported_at > *t {
          *t = point.reported_at;
        }
      })
      .or_insert(point.reported_at);
  }
  latest
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::macros::datetime;

  fn config(raw: &str) -> JsonString {
    JsonString::new(raw.to_string()).expect("test fixture must be valid JSON")
  }

  #[test]
  fn interval_reads_the_configured_value() {
    let cfg = config(r#"{"telemetry_interval":60,"logging":"info"}"#);
    assert_eq!(telemetry_interval_secs(&cfg), Some(60));
  }

  #[test]
  fn interval_is_none_when_key_missing() {
    let cfg = config(r#"{"logging":"info"}"#);
    assert_eq!(telemetry_interval_secs(&cfg), None);
  }

  #[test]
  fn interval_is_none_for_malformed_json() {
    let cfg = config("{}");
    // Sanity: valid-but-empty JSON just has no key -- malformed JSON is
    // not constructible via JsonString::new at all (mirrors
    // extract_firmware_target's contract).
    assert_eq!(telemetry_interval_secs(&cfg), None);
  }

  #[test]
  fn never_reported_true_for_fresh_pigeon_defaults() {
    assert!(has_never_reported(0, &config("{}")));
  }

  #[test]
  fn never_reported_false_once_version_bumps() {
    // A real device report bumps current_version even if the config it
    // echoed back happens to still be empty (e.g. echoing an empty
    // target_config).
    assert!(!has_never_reported(1, &config("{}")));
  }

  #[test]
  fn never_reported_false_once_config_is_non_trivial() {
    // Belt-and-suspenders: a non-empty current_config alone (regardless of
    // version) means something real got written back.
    assert!(!has_never_reported(0, &config(r#"{"telemetry_interval":60}"#)));
  }

  #[test]
  fn latest_of_picks_the_newest_present_signal() {
    let a = Some(datetime!(2026-07-18 10:00:00 UTC));
    let b = Some(datetime!(2026-07-18 12:00:00 UTC));
    let c = None;
    assert_eq!(latest_of([a, b, c]), Some(datetime!(2026-07-18 12:00:00 UTC)));
  }

  #[test]
  fn latest_of_all_none_is_none() {
    assert_eq!(latest_of([None, None]), None);
  }

  #[test]
  fn classify_never_seen_is_unknown() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    assert_eq!(classify(None, Some(60), now), ConnectionState::Unknown);
  }

  #[test]
  fn classify_within_2x_interval_is_online() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    let seen = now - time::Duration::seconds(90); // interval 60s, 2x = 120s
    assert_eq!(classify(Some(seen), Some(60), now), ConnectionState::Online);
  }

  #[test]
  fn classify_between_2x_and_5x_interval_is_stale() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    let seen = now - time::Duration::seconds(200); // interval 60s: 120s < 200s <= 300s
    assert_eq!(classify(Some(seen), Some(60), now), ConnectionState::Stale);
  }

  #[test]
  fn classify_beyond_5x_interval_is_offline() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    let seen = now - time::Duration::seconds(301); // interval 60s, 5x = 300s
    assert_eq!(classify(Some(seen), Some(60), now), ConnectionState::Offline);
  }

  #[test]
  fn classify_falls_back_to_fixed_thresholds_with_no_interval() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    let just_online = now - time::Duration::seconds(DEFAULT_ONLINE_SECS);
    let just_stale = now - time::Duration::seconds(DEFAULT_ONLINE_SECS + 1);
    let just_offline = now - time::Duration::seconds(DEFAULT_STALE_SECS + 1);
    assert_eq!(classify(Some(just_online), None, now), ConnectionState::Online);
    assert_eq!(classify(Some(just_stale), None, now), ConnectionState::Stale);
    assert_eq!(classify(Some(just_offline), None, now), ConnectionState::Offline);
  }

  #[test]
  fn classify_treats_future_timestamps_as_now() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    let future = now + time::Duration::seconds(120); // device clock skew ahead
    assert_eq!(classify(Some(future), Some(60), now), ConnectionState::Online);
  }

  #[test]
  fn format_last_seen_never() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    assert_eq!(format_last_seen(None, now), "Never seen");
  }

  #[test]
  fn format_last_seen_buckets() {
    let now = datetime!(2026-07-18 12:00:00 UTC);
    assert_eq!(
      format_last_seen(Some(now - time::Duration::seconds(10)), now),
      "just now"
    );
    assert_eq!(
      format_last_seen(Some(now - time::Duration::minutes(5)), now),
      "5m ago"
    );
    assert_eq!(
      format_last_seen(Some(now - time::Duration::hours(3)), now),
      "3h ago"
    );
    assert_eq!(
      format_last_seen(Some(now - time::Duration::days(2)), now),
      "2d ago"
    );
  }

  #[test]
  fn latest_seen_by_pigeon_takes_the_max_per_pigeon() {
    fn point(pigeon_id: &str, reported_at: OffsetDateTime) -> TelemetryHistoryPoint {
      TelemetryHistoryPoint {
        pigeon_id: pigeon_id.to_string(),
        key: "battery_mv".to_string(),
        value: "4000".to_string(),
        value_num: Some(4000.0),
        reported_at,
      }
    }
    let t1 = datetime!(2026-07-18 10:00:00 UTC);
    let t2 = datetime!(2026-07-18 11:00:00 UTC);
    let points = vec![point("a", t1), point("a", t2), point("b", t1)];
    let latest = latest_seen_by_pigeon(&points);
    assert_eq!(latest.get("a"), Some(&t2));
    assert_eq!(latest.get("b"), Some(&t1));
    assert_eq!(latest.get("c"), None);
  }
}
