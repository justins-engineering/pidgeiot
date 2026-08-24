//! Pure logic behind the pigeon Durable Object's latest-value telemetry
//! store. The store is one JSON object -- key -> {value, reported_at} --
//! living in a single SQLite row, rather than a row per key. Durable Object
//! SQLite bills per row read and written, and telemetry ingest is the
//! hottest write path on the platform (every report from every device, over
//! HTTP and WebSocket alike, with a third ingress on the way), so a ten-key
//! report costs one row read plus one row write here instead of a full table
//! scan plus ten upserts.
//!
//! Nothing in this module touches `worker`, so all of it runs under a plain
//! `cargo test -p dovecote`.

use crate::objects::pigeons::PreviousTelemetryValue;
use capsules::{
  MAX_TELEMETRY_KEY_BYTES, MAX_TELEMETRY_KEYS, MAX_TELEMETRY_VALUE_BYTES, TelemetryLatest,
};
use std::collections::{HashMap, HashSet};

/// The decoded latest-value store. `PreviousTelemetryValue` is reused as the
/// per-key entry deliberately: a key's stored entry *is* what the next report
/// hands the alert evaluator as that key's previous value, so a second
/// identical struct would only be two names for one shape.
pub type TelemetryBlob = HashMap<String, PreviousTelemetryValue>;

/// A row of the retired per-key `pigeon_telemetry` table, read once during a
/// DO's lazy migration (see `fold_legacy_rows`). SQLite hands `reported_at`
/// back as an integer, same as the other DO row types.
#[derive(serde::Deserialize)]
pub struct LegacyTelemetryRow {
  pub key: String,
  pub value: String,
  pub reported_at: i64,
}

/// Checks a report before any of it is written, so an oversized one is
/// refused whole rather than partly applied. The messages describe the limit
/// only -- naming the offending key would reflect device-supplied bytes back
/// into a response body for no diagnostic gain the limit itself doesn't give.
pub fn validate_telemetry_report(metrics: &HashMap<String, String>) -> Result<(), String> {
  if metrics.len() > MAX_TELEMETRY_KEYS {
    return Err(format!(
      "Bad Request: telemetry report carries more than {MAX_TELEMETRY_KEYS} keys"
    ));
  }

  for (key, value) in metrics {
    if key.is_empty() {
      return Err("Bad Request: empty telemetry key".into());
    }
    if key.len() > MAX_TELEMETRY_KEY_BYTES {
      return Err(format!(
        "Bad Request: telemetry key longer than {MAX_TELEMETRY_KEY_BYTES} bytes"
      ));
    }
    if value.len() > MAX_TELEMETRY_VALUE_BYTES {
      return Err(format!(
        "Bad Request: telemetry value longer than {MAX_TELEMETRY_VALUE_BYTES} bytes"
      ));
    }
  }

  Ok(())
}

pub fn parse_blob(text: &str) -> Result<TelemetryBlob, serde_json::Error> {
  serde_json::from_str(text)
}

/// Folds the retired per-key table's rows into the blob during a DO's lazy
/// migration. Blob entries win over legacy rows: the migration writes the
/// blob before dropping the legacy table, so a rerun that finds both is
/// resuming an interrupted migration, and anything already in the blob is
/// either the same data or newer than what the legacy table holds.
pub fn fold_legacy_rows(blob: &mut TelemetryBlob, rows: Vec<LegacyTelemetryRow>) {
  for row in rows {
    blob.entry(row.key).or_insert(PreviousTelemetryValue {
      value: row.value,
      reported_at: row.reported_at,
    });
  }

  evict_to_cap(blob, &HashSet::new());
}

/// Drops least-recently-reported keys until the blob is back within
/// `MAX_TELEMETRY_KEYS`. Keys carried by the report that triggered this are
/// never candidates: a device that reports more distinct keys than it used to
/// should shed its abandoned keys, not the ones it just sent. Ties break on
/// the key name so the outcome is deterministic rather than hash-order.
///
/// `protected` is a key set rather than the report itself because a batched
/// report protects the union of every reading's keys, not any one reading's
/// (`helpers::telemetry_batch::merge_batch`).
pub(crate) fn evict_to_cap(blob: &mut TelemetryBlob, protected: &HashSet<&str>) {
  if blob.len() <= MAX_TELEMETRY_KEYS {
    return;
  }

  let mut candidates: Vec<(i64, String)> = blob
    .iter()
    .filter(|(key, _)| !protected.contains(key.as_str()))
    .map(|(key, entry)| (entry.reported_at, key.clone()))
    .collect();
  candidates.sort();

  let excess = blob.len() - MAX_TELEMETRY_KEYS;
  for (_, key) in candidates.into_iter().take(excess) {
    blob.remove(&key);
  }
}

/// The public read shape, sorted by key so the dashboard sees a stable order
/// across requests instead of hash order.
pub fn to_latest(blob: &TelemetryBlob) -> Vec<TelemetryLatest> {
  let mut latest: Vec<TelemetryLatest> = blob
    .iter()
    .map(|(key, entry)| {
      TelemetryLatest::from_unix_seconds(key.clone(), entry.value.clone(), entry.reported_at)
    })
    .collect();
  latest.sort_by(|a, b| a.key.cmp(&b.key));
  latest
}

#[cfg(test)]
mod tests {
  use super::*;

  fn metrics(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
      .iter()
      .map(|(k, v)| (k.to_string(), v.to_string()))
      .collect()
  }

  fn blob_of(entries: &[(&str, &str, i64)]) -> TelemetryBlob {
    entries
      .iter()
      .map(|(k, v, t)| {
        (
          k.to_string(),
          PreviousTelemetryValue {
            value: v.to_string(),
            reported_at: *t,
          },
        )
      })
      .collect()
  }

  #[test]
  fn validation_refuses_a_report_over_the_key_cap() {
    let report: HashMap<String, String> = (0..MAX_TELEMETRY_KEYS + 1)
      .map(|i| (format!("k{i}"), "1".to_string()))
      .collect();
    assert!(validate_telemetry_report(&report).is_err());

    let at_cap: HashMap<String, String> = (0..MAX_TELEMETRY_KEYS)
      .map(|i| (format!("k{i}"), "1".to_string()))
      .collect();
    assert!(validate_telemetry_report(&at_cap).is_ok());
  }

  #[test]
  fn validation_refuses_oversize_keys_values_and_empty_keys() {
    let long_key = "k".repeat(MAX_TELEMETRY_KEY_BYTES + 1);
    assert!(validate_telemetry_report(&metrics(&[(&long_key, "1")])).is_err());

    let long_value = "v".repeat(MAX_TELEMETRY_VALUE_BYTES + 1);
    assert!(validate_telemetry_report(&metrics(&[("k", &long_value)])).is_err());

    assert!(validate_telemetry_report(&metrics(&[("", "1")])).is_err());

    let at_bounds = metrics(&[("k", &"v".repeat(MAX_TELEMETRY_VALUE_BYTES))]);
    assert!(validate_telemetry_report(&at_bounds).is_ok());
  }

  #[test]
  fn legacy_rows_fold_in_with_their_own_timestamps() {
    let mut blob = TelemetryBlob::new();
    fold_legacy_rows(
      &mut blob,
      vec![
        LegacyTelemetryRow {
          key: "reset_cause".into(),
          value: "power_on".into(),
          reported_at: 1000,
        },
        LegacyTelemetryRow {
          key: "uptime_s".into(),
          value: "70".into(),
          reported_at: 1060,
        },
      ],
    );

    assert_eq!(blob.len(), 2);
    assert_eq!(blob["reset_cause"].reported_at, 1000);
    assert_eq!(blob["uptime_s"].value, "70");
  }

  #[test]
  fn a_resumed_migration_keeps_the_blob_and_folds_only_what_is_missing() {
    let mut blob = blob_of(&[("uptime_s", "700", 2000)]);
    fold_legacy_rows(
      &mut blob,
      vec![
        LegacyTelemetryRow {
          key: "uptime_s".into(),
          value: "70".into(),
          reported_at: 1060,
        },
        LegacyTelemetryRow {
          key: "reset_cause".into(),
          value: "power_on".into(),
          reported_at: 1000,
        },
      ],
    );

    assert_eq!(blob["uptime_s"].value, "700");
    assert_eq!(blob["uptime_s"].reported_at, 2000);
    assert_eq!(blob["reset_cause"].value, "power_on");
  }

  #[test]
  fn a_legacy_table_over_the_cap_folds_down_to_its_newest_keys() {
    let mut blob = TelemetryBlob::new();
    let rows: Vec<LegacyTelemetryRow> = (0..MAX_TELEMETRY_KEYS + 10)
      .map(|i| LegacyTelemetryRow {
        key: format!("k{i:03}"),
        value: "x".into(),
        reported_at: i as i64,
      })
      .collect();
    fold_legacy_rows(&mut blob, rows);

    assert_eq!(blob.len(), MAX_TELEMETRY_KEYS);
    assert!(!blob.contains_key("k000"));
    assert!(blob.contains_key("k137"));
  }

  #[test]
  fn a_round_trip_through_storage_keeps_values_and_timestamps() {
    let mut blob = blob_of(&[("temp", "21.5", 500)]);
    blob.insert(
      "status".to_string(),
      PreviousTelemetryValue {
        value: "ok".to_string(),
        reported_at: 900,
      },
    );

    let stored = serde_json::to_string(&blob).unwrap();
    let restored = parse_blob(&stored).unwrap();

    let latest = to_latest(&restored);
    assert_eq!(latest.len(), 2);
    // Sorted by key: status before temp.
    assert_eq!(latest[0].key, "status");
    assert_eq!(latest[0].reported_at.unix_timestamp(), 900);
    assert_eq!(latest[1].key, "temp");
    assert_eq!(latest[1].value, "21.5");
    assert_eq!(latest[1].reported_at.unix_timestamp(), 500);
  }

  #[test]
  fn an_unreadable_blob_is_an_error_rather_than_a_silent_empty_store() {
    assert!(parse_blob("not json").is_err());
    assert!(parse_blob("{}").unwrap().is_empty());
  }
}
