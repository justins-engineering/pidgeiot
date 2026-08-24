//! Pure logic behind batched telemetry reports: validating a batch's
//! caps, resolving its device-supplied timestamps against the server's own
//! clock, and merging the whole batch into the latest-value store in one
//! pass.
//!
//! Batching exists for cost, not convenience. A device reporting every ten
//! seconds books 259,200 reports a month, and on the HTTPS path each one
//! costs a worker request, two Durable Object round trips, three queue
//! operations and a Durable Object row write. Folding M readings into one
//! delivery divides every one of those by M while the stored data comes
//! out identical, because the per-reading cost that survives -- a history
//! row, a line of line protocol, a billable message -- is the part that
//! measures readings rather than envelopes.
//!
//! Nothing here touches `worker`, so all of it runs under a plain
//! `cargo test -p dovecote`. The one thing this module deliberately does
//! not do is decide what "now" is: every entry point takes it as an
//! argument, which is what makes clamping testable.

use crate::helpers::telemetry_latest::{TelemetryBlob, evict_to_cap, validate_telemetry_report};
use crate::objects::pigeons::PreviousTelemetryValue;
use capsules::{
  MAX_TELEMETRY_BACKDATE_SECS, MAX_TELEMETRY_BATCH_READINGS, MAX_TELEMETRY_KEYS, TelemetryBatch,
  TelemetryReading, TelemetryReportBody,
};
use std::collections::{HashMap, HashSet};

/// One reading with its timestamp already resolved and clamped, which is
/// the only form the rest of the pipeline ever sees -- the queue message,
/// the Durable Object write, the history insert and the line protocol all
/// take these, so a device-supplied timestamp can only enter through
/// `resolve_batch` below.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct ResolvedReading {
  /// Unix seconds, matching how the latest-value store stamps every key.
  pub at_secs: i64,
  pub metrics: HashMap<String, String>,
  /// Each key's stored entry from immediately before this reading
  /// overwrote it, for `RateOfChange`. Populated by `merge_batch` and
  /// carried on the queue message only where the merge already happened at
  /// ingest time (the WebSocket path); `None` everywhere the queue
  /// consumer's own Durable Object write will fill it in.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub previous: Option<HashMap<String, PreviousTelemetryValue>>,
}

impl ResolvedReading {
  pub fn new(at_secs: i64, metrics: HashMap<String, String>) -> Self {
    Self {
      at_secs,
      metrics,
      previous: None,
    }
  }
}

/// Resolves a device's timestamp claim into server time. Device timestamps
/// are advisory in both spellings: `age_secs` because the device it exists
/// for has no wall clock to be right about, `at` because a client's clock
/// being wrong must not be able to move a reading outside the window the
/// server is willing to accept.
///
/// The clamp is asymmetric on purpose. The future side clamps to `now`
/// exactly: a reading stamped ahead of the present would sit beyond the
/// end of every history range a dashboard asks for, and would let a device
/// with a fast clock post readings into a billing period that has not
/// started. The past side clamps to `MAX_TELEMETRY_BACKDATE_SECS`, which
/// bounds how far a device can rewrite history while still letting a long
/// offline buffer land.
fn resolve_at(reading: &TelemetryReading, now_secs: i64) -> i64 {
  let claimed = match (reading.age_secs, reading.at) {
    // `age_secs` wins over `at`: a client filling it in is saying it does
    // not trust its own clock, and the relative form survives a wrong one.
    (Some(age), _) => now_secs - age.max(0),
    (None, Some(at)) => at,
    (None, None) => now_secs,
  };

  claimed.clamp(now_secs - MAX_TELEMETRY_BACKDATE_SECS, now_secs)
}

/// Every distinct key across a batch. Bounding this, rather than only each
/// reading's own key count, is what keeps eviction well defined: the merge
/// protects the batch's own keys from eviction, so a batch whose union
/// exceeded the store's capacity could only be applied by evicting keys it
/// had just delivered.
fn batch_key_union(reports: &[TelemetryReading]) -> HashSet<&str> {
  reports
    .iter()
    .flat_map(|r| r.metrics.keys().map(String::as_str))
    .collect()
}

/// Validates a batch whole and resolves it into chronological order.
/// Refused whole rather than partly applied, exactly as an over-cap flat
/// report is: half a batch is worse than none, because a device that got a
/// success for it has no way to learn which half to resend.
///
/// The sort is stable, so readings a device stamped identically keep the
/// order it sent them in -- which is the order it sampled them, and the
/// only ordering information left once two readings share a second.
pub fn resolve_batch(
  batch: &TelemetryBatch,
  now_secs: i64,
) -> Result<Vec<ResolvedReading>, String> {
  if batch.reports.is_empty() {
    return Err("Bad Request: Empty telemetry report".into());
  }

  if batch.reports.len() > MAX_TELEMETRY_BATCH_READINGS {
    return Err(format!(
      "Bad Request: telemetry batch carries more than {MAX_TELEMETRY_BATCH_READINGS} readings"
    ));
  }

  for reading in &batch.reports {
    if reading.metrics.is_empty() {
      return Err("Bad Request: Empty telemetry report".into());
    }
    validate_telemetry_report(&reading.metrics)?;
  }

  if batch_key_union(&batch.reports).len() > MAX_TELEMETRY_KEYS {
    return Err(format!(
      "Bad Request: telemetry batch spans more than {MAX_TELEMETRY_KEYS} distinct keys"
    ));
  }

  let mut resolved: Vec<ResolvedReading> = batch
    .reports
    .iter()
    .map(|reading| ResolvedReading::new(resolve_at(reading, now_secs), reading.metrics.clone()))
    .collect();

  resolved.sort_by_key(|reading| reading.at_secs);
  Ok(resolved)
}

/// Merges a whole batch into the store in one pass, stamping each key with
/// its own reading's resolved timestamp and filling in each reading's
/// `previous` as it goes.
///
/// Two things make this more than `merge_report` in a loop. Within the
/// batch, a key's "previous value" is whatever an *earlier reading in the
/// same batch* left, not what the store held before the batch began --
/// without that, every reading after the first would diff against a value
/// several readings stale and `RateOfChange` would see one giant step
/// instead of the progression that actually happened. And a reading older
/// than what the store already holds for a key never overwrites it: a
/// backdated batch arriving after a live report must not drag the
/// latest-value store backwards, since the whole store's contract is that
/// it holds the newest value per key.
///
/// Eviction runs once, at the end, against the batch's whole key union --
/// running it per reading could evict a key that a later reading in the
/// same batch was about to refresh.
pub fn merge_batch(blob: &mut TelemetryBlob, readings: &mut [ResolvedReading]) {
  for reading in readings.iter_mut() {
    let mut previous = HashMap::new();

    for (key, value) in &reading.metrics {
      if let Some(entry) = blob.get(key) {
        previous.insert(key.clone(), entry.clone());

        // Strictly newer, so a reading sharing a second with the stored
        // entry still wins -- within one second, later arrival is the only
        // ordering signal there is.
        if entry.reported_at > reading.at_secs {
          continue;
        }
      }

      blob.insert(
        key.clone(),
        PreviousTelemetryValue {
          value: value.clone(),
          reported_at: reading.at_secs,
        },
      );
    }

    reading.previous = Some(previous);
  }

  let protected: HashSet<&str> = readings
    .iter()
    .flat_map(|reading| reading.metrics.keys().map(String::as_str))
    .collect();
  evict_to_cap(blob, &protected);
}

/// A device-supplied telemetry body, already parsed, turned into the
/// chronological readings every write path takes. The flat form is one
/// reading stamped now; the batch form resolves each reading's own
/// device-supplied timestamp against this server's clock (see
/// `helpers::resolve_batch` for the clamping rules).
///
/// Every entry point resolves through here -- the gateway route before it
/// enqueues, the no-queue direct write, and the WebSocket frame -- so a
/// device timestamp can never reach the store without passing the clamp.
pub fn readings_from_body(
  body: TelemetryReportBody,
  now_secs: i64,
) -> Result<Vec<ResolvedReading>, String> {
  match body {
    TelemetryReportBody::Batch(batch) => resolve_batch(&batch, now_secs),
    TelemetryReportBody::Flat(metrics) => {
      if metrics.is_empty() {
        return Err("Bad Request: Empty telemetry report".into());
      }
      validate_telemetry_report(&metrics)?;
      Ok(vec![ResolvedReading::new(now_secs, metrics)])
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const NOW: i64 = 1_800_000_000;

  fn metrics(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
      .iter()
      .map(|(k, v)| (k.to_string(), v.to_string()))
      .collect()
  }

  fn aged(age_secs: i64, pairs: &[(&str, &str)]) -> TelemetryReading {
    TelemetryReading {
      at: None,
      age_secs: Some(age_secs),
      metrics: metrics(pairs),
    }
  }

  fn stamped(at: i64, pairs: &[(&str, &str)]) -> TelemetryReading {
    TelemetryReading {
      at: Some(at),
      age_secs: None,
      metrics: metrics(pairs),
    }
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
  fn relative_ages_resolve_backwards_from_the_receive_time() {
    let batch = TelemetryBatch {
      reports: vec![
        aged(50, &[("temp", "1")]),
        aged(0, &[("temp", "6")]),
        aged(20, &[("temp", "4")]),
      ],
    };

    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert_eq!(
      resolved.iter().map(|r| r.at_secs).collect::<Vec<_>>(),
      vec![NOW - 50, NOW - 20, NOW]
    );
    // Sorted chronologically, so the values follow the timestamps.
    assert_eq!(resolved[0].metrics["temp"], "1");
    assert_eq!(resolved[2].metrics["temp"], "6");
  }

  #[test]
  fn out_of_order_absolute_timestamps_sort_chronologically() {
    let batch = TelemetryBatch {
      reports: vec![
        stamped(NOW - 10, &[("seq", "b")]),
        stamped(NOW - 30, &[("seq", "a")]),
        stamped(NOW - 5, &[("seq", "c")]),
      ],
    };

    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert_eq!(
      resolved
        .iter()
        .map(|r| r.metrics["seq"].as_str())
        .collect::<Vec<_>>(),
      vec!["a", "b", "c"]
    );
  }

  #[test]
  fn a_future_timestamp_clamps_to_the_receive_time() {
    let batch = TelemetryBatch {
      reports: vec![stamped(NOW + 86_400, &[("temp", "1")])],
    };
    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert_eq!(resolved[0].at_secs, NOW);
  }

  #[test]
  fn a_timestamp_older_than_the_backdate_window_clamps_to_the_boundary() {
    let batch = TelemetryBatch {
      reports: vec![
        stamped(NOW - MAX_TELEMETRY_BACKDATE_SECS - 5_000, &[("temp", "1")]),
        aged(MAX_TELEMETRY_BACKDATE_SECS + 5_000, &[("temp", "2")]),
      ],
    };
    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert!(
      resolved
        .iter()
        .all(|r| r.at_secs == NOW - MAX_TELEMETRY_BACKDATE_SECS)
    );
  }

  #[test]
  fn a_negative_age_is_treated_as_now_rather_than_the_future() {
    let batch = TelemetryBatch {
      reports: vec![aged(-600, &[("temp", "1")])],
    };
    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert_eq!(resolved[0].at_secs, NOW);
  }

  #[test]
  fn a_reading_with_neither_timestamp_form_lands_at_the_receive_time() {
    let batch = TelemetryBatch {
      reports: vec![TelemetryReading {
        at: None,
        age_secs: None,
        metrics: metrics(&[("temp", "1")]),
      }],
    };
    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert_eq!(resolved[0].at_secs, NOW);
  }

  #[test]
  fn a_relative_age_wins_over_an_absolute_timestamp() {
    let batch = TelemetryBatch {
      reports: vec![TelemetryReading {
        at: Some(NOW - 9_000),
        age_secs: Some(30),
        metrics: metrics(&[("temp", "1")]),
      }],
    };
    let resolved = resolve_batch(&batch, NOW).expect("batch should resolve");
    assert_eq!(resolved[0].at_secs, NOW - 30);
  }

  #[test]
  fn an_over_cap_batch_is_refused_whole() {
    let batch = TelemetryBatch {
      reports: (0..MAX_TELEMETRY_BATCH_READINGS + 1)
        .map(|i| aged(i as i64, &[("temp", "1")]))
        .collect(),
    };
    assert!(resolve_batch(&batch, NOW).is_err());

    let at_cap = TelemetryBatch {
      reports: (0..MAX_TELEMETRY_BATCH_READINGS)
        .map(|i| aged(i as i64, &[("temp", "1")]))
        .collect(),
    };
    assert!(resolve_batch(&at_cap, NOW).is_ok());
  }

  #[test]
  fn an_empty_batch_and_an_empty_reading_are_both_refused() {
    assert!(resolve_batch(&TelemetryBatch { reports: vec![] }, NOW).is_err());
    assert!(
      resolve_batch(
        &TelemetryBatch {
          reports: vec![aged(0, &[])],
        },
        NOW
      )
      .is_err()
    );
  }

  #[test]
  fn a_batch_spanning_more_keys_than_the_store_holds_is_refused() {
    // Each reading is well inside the per-reading cap; only the union
    // exceeds it, which is exactly the case a per-reading check misses.
    let reports: Vec<TelemetryReading> = (0..MAX_TELEMETRY_KEYS + 1)
      .map(|i| {
        let key = format!("k{i:03}");
        TelemetryReading {
          at: None,
          age_secs: Some(i as i64),
          metrics: metrics(&[(key.as_str(), "1")]),
        }
      })
      .collect();
    assert!(resolve_batch(&TelemetryBatch { reports }, NOW).is_err());
  }

  #[test]
  fn a_batch_inherits_the_per_reading_key_and_value_caps() {
    let long_value = "v".repeat(capsules::MAX_TELEMETRY_VALUE_BYTES + 1);
    let batch = TelemetryBatch {
      reports: vec![
        aged(10, &[("temp", "1")]),
        aged(0, &[("temp", long_value.as_str())]),
      ],
    };
    assert!(resolve_batch(&batch, NOW).is_err());
  }

  #[test]
  fn merging_a_batch_leaves_the_newest_value_per_key() {
    let mut blob = blob_of(&[("temp", "20.0", NOW - 100)]);
    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![
          aged(30, &[("temp", "21.0")]),
          aged(10, &[("temp", "23.0")]),
          aged(20, &[("temp", "22.0")]),
        ],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert_eq!(blob["temp"].value, "23.0");
    assert_eq!(blob["temp"].reported_at, NOW - 10);
  }

  #[test]
  fn each_reading_sees_the_one_before_it_as_its_previous_value() {
    // The progression an alert has to see: three 5-degree steps, not one
    // 15-degree jump from the pre-batch value.
    let mut blob = blob_of(&[("temp", "20", NOW - 100)]);
    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![
          aged(30, &[("temp", "25")]),
          aged(20, &[("temp", "30")]),
          aged(10, &[("temp", "35")]),
        ],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert_eq!(
      readings[0].previous.clone().unwrap_or_default()["temp"].value,
      "20"
    );
    assert_eq!(
      readings[0].previous.clone().unwrap_or_default()["temp"].reported_at,
      NOW - 100
    );
    assert_eq!(
      readings[1].previous.clone().unwrap_or_default()["temp"].value,
      "25"
    );
    assert_eq!(
      readings[1].previous.clone().unwrap_or_default()["temp"].reported_at,
      NOW - 30
    );
    assert_eq!(
      readings[2].previous.clone().unwrap_or_default()["temp"].value,
      "30"
    );
    assert_eq!(
      readings[2].previous.clone().unwrap_or_default()["temp"].reported_at,
      NOW - 20
    );
  }

  #[test]
  fn a_keys_first_appearance_in_a_batch_has_no_previous_value() {
    let mut blob = TelemetryBlob::new();
    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![aged(10, &[("temp", "25")]), aged(0, &[("temp", "26")])],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert!(
      !readings[0]
        .previous
        .clone()
        .unwrap_or_default()
        .contains_key("temp")
    );
    assert_eq!(
      readings[1].previous.clone().unwrap_or_default()["temp"].value,
      "25"
    );
  }

  #[test]
  fn a_batch_stamps_only_the_keys_each_reading_carries() {
    let mut blob = blob_of(&[("reset_cause", "power_on", NOW - 9_000)]);
    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![aged(20, &[("uptime_s", "10")]), aged(0, &[("temp", "21")])],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert_eq!(blob["reset_cause"].reported_at, NOW - 9_000);
    assert_eq!(blob["uptime_s"].reported_at, NOW - 20);
    assert_eq!(blob["temp"].reported_at, NOW);
  }

  #[test]
  fn a_backdated_batch_never_drags_a_key_backwards() {
    // A live report already landed at NOW; a batch buffered from before it
    // arrives afterwards. History gets every reading, but the
    // latest-value store keeps the newest one.
    let mut blob = blob_of(&[("temp", "30", NOW)]);
    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![aged(600, &[("temp", "10")]), aged(300, &[("temp", "20")])],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert_eq!(blob["temp"].value, "30");
    assert_eq!(blob["temp"].reported_at, NOW);
    // The stale readings still report their true previous values, so
    // history and alerts see the progression the device actually sampled.
    assert_eq!(
      readings[0].previous.clone().unwrap_or_default()["temp"].value,
      "30"
    );
  }

  #[test]
  fn a_reading_sharing_a_second_with_the_stored_entry_still_wins() {
    let mut blob = blob_of(&[("temp", "30", NOW)]);
    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![aged(0, &[("temp", "31")])],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert_eq!(blob["temp"].value, "31");
  }

  #[test]
  fn eviction_protects_every_key_the_batch_carried() {
    let mut blob: TelemetryBlob = (0..MAX_TELEMETRY_KEYS)
      .map(|i| {
        (
          format!("old_{i:03}"),
          PreviousTelemetryValue {
            value: "x".into(),
            reported_at: i as i64,
          },
        )
      })
      .collect();

    let mut readings = resolve_batch(
      &TelemetryBatch {
        reports: vec![aged(10, &[("fresh_a", "1")]), aged(0, &[("fresh_b", "2")])],
      },
      NOW,
    )
    .expect("batch should resolve");

    merge_batch(&mut blob, &mut readings);

    assert_eq!(blob.len(), MAX_TELEMETRY_KEYS);
    assert!(blob.contains_key("fresh_a"));
    assert!(blob.contains_key("fresh_b"));
    assert!(!blob.contains_key("old_000"));
    assert!(!blob.contains_key("old_001"));
    assert!(blob.contains_key("old_002"));
  }
}
