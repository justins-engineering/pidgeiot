//! A rule's dry run: the test cases the test route accepts, and the result it answers and
//! `rule_versions.test_summary` stores. Nothing a dry run reports was applied: no mail, state or
//! value.

use crate::{RuleAlertKind, RuleSeverity};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Most mails one replayed version lists, oldest first; `mails_total` counts them all.
pub const MAX_DRY_RUN_MAILS: usize = 100;

/// Most error groups one replayed version lists, the most frequent first.
pub const MAX_DRY_RUN_ERRORS: usize = 10;

/// Most log lines one replayed version keeps, from its earliest runs that logged.
pub const MAX_DRY_RUN_LOGS: usize = 20;

/// Two mails for one pigeon, key and kind this close in time are the same mail when the
/// candidate's are matched against the deployed version's.
pub const DRY_RUN_MAIL_MATCH_SECS: i64 = 300;

/// Most bytes of a test route body, checked before it is parsed: room for five cases of eight
/// batches, each the size a device's batch may be ([`MAX_TELEMETRY_BATCH_BYTES`]), in the
/// wordier key and value pairs a case spells metrics in.
///
/// [`MAX_TELEMETRY_BATCH_BYTES`]: crate::MAX_TELEMETRY_BATCH_BYTES
pub const MAX_RULE_TEST_BODY_BYTES: usize = 1024 * 1024;

/// Body of `POST .../versions/:version/test`; may be omitted.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct RuleTestRequest {
  /// Synthetic cases replayed before the recorded telemetry: none, or two to five.
  #[serde(default)]
  pub cases: Vec<RuleTestCase>,
}

/// One synthetic test case: batches from up to three abstract pigeons, each case starting from
/// empty state, and the mails the platform should send over it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleTestCase {
  /// Shown beside the case's outcome.
  pub name: String,
  /// Batches in the order they arrive.
  pub events: Vec<RuleCaseEvent>,
  /// The mails the platform should send over the whole case.
  pub expect: Vec<RuleCaseMail>,
}

/// One batch of a test case.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleCaseEvent {
  /// Which of the three abstract pigeons sent it.
  pub pigeon: RuleCasePigeon,
  /// Its readings, in any order.
  pub readings: Vec<RuleCaseReading>,
}

/// The three abstract pigeons a case may name; a case never names a real id.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleCasePigeon {
  /// Pigeon "a".
  A,
  /// Pigeon "b".
  B,
  /// Pigeon "c".
  C,
}

/// One reading of a test case.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleCaseReading {
  /// Seconds after the case's start.
  pub after_secs: i64,
  /// Key and value pairs, since a model's structured output refuses a map-shaped object.
  pub metrics: Vec<RuleCaseMetric>,
}

/// One metric of a case reading.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleCaseMetric {
  /// The telemetry key.
  pub key: String,
  /// Its value, a string like every telemetry value.
  pub value: String,
}

/// A mail by its alert key and kind, as a case expects it and as its outcome reports it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuleCaseMail {
  /// The alert key.
  pub key: String,
  /// Its kind.
  pub kind: RuleAlertKind,
}

/// A whole dry run.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleDryRun {
  /// What telemetry was replayed.
  pub window: RuleReplayWindow,
  /// The tested version over the window, from empty state.
  pub candidate: RuleReplayed,
  /// The deployed version over the same window, when there is one and it is another version.
  pub deployed: Option<RuleReplayed>,
  /// The tested version once more, seeded with the state the live rule stores, when it stores
  /// any: a failure only here is what "reset state on deploy" avoids.
  pub seeded: Option<RuleReplayed>,
  /// The candidate's mails against the deployed version's; all new without one.
  pub mails: RuleMailChange,
  /// One outcome per synthetic case, in the order given.
  pub cases: Vec<RuleCaseOutcome>,
  /// The CPU check under the live limit, a warning only.
  pub cpu: RuleCpuCheck,
}

/// The telemetry a dry run replayed: the flock's recent history, field pigeons never read.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleReplayWindow {
  /// The oldest reading replayed; `None` when there was nothing to replay.
  #[serde(with = "time::serde::rfc3339::option")]
  pub from: Option<OffsetDateTime>,
  /// The newest reading replayed.
  #[serde(with = "time::serde::rfc3339::option")]
  pub to: Option<OffsetDateTime>,
  /// Readings replayed.
  pub readings: u32,
  /// Batches they were grouped into, each one run of the rule.
  pub batches: u32,
  /// Pigeons they came from.
  pub pigeons: u32,
  /// Older readings inside the window were left out to stay under the replay's cap.
  pub cut: bool,
}

/// One version's replay.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleReplayed {
  /// The version replayed.
  pub version: i32,
  /// Batches it ran over; fewer than the window's when a timeout stopped it.
  pub runs: u32,
  /// Runs with a valid result.
  pub ok: u32,
  /// Runs that threw.
  pub threw: u32,
  /// Runs that ran out of time; the first stops the replay.
  pub timeout: u32,
  /// Results the platform would refuse.
  pub bad_output: u32,
  /// Derived values it would have written.
  pub values: u32,
  /// Mails it would have sent, oldest first, at most [`MAX_DRY_RUN_MAILS`].
  pub mails: Vec<RuleDryMail>,
  /// Every mail, listed or not.
  pub mails_total: u32,
  /// Its errors grouped by text, at most [`MAX_DRY_RUN_ERRORS`].
  pub errors: Vec<RuleErrorGroup>,
  /// The first [`MAX_DRY_RUN_LOGS`] lines it logged.
  pub logs: Vec<String>,
  /// The batch at which a timeout stopped the replay.
  #[serde(with = "time::serde::rfc3339::option")]
  pub stopped_at: Option<OffsetDateTime>,
  /// The first result the platform refuses that the harness took: later runs saw state that live
  /// would not have stored, so they may differ from live.
  #[serde(with = "time::serde::rfc3339::option")]
  pub diverged_at: Option<OffsetDateTime>,
  /// The largest pigeon state document it left, in bytes.
  pub pigeon_state_bytes: u32,
  /// The flock state document it left, in bytes.
  pub flock_state_bytes: u32,
}

/// Runs that failed with one text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleErrorGroup {
  /// The rule's own error text, or the platform's refusal of its result.
  pub message: String,
  /// Runs that failed with it.
  pub count: u32,
}

/// A mail a replay would have sent.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleDryMail {
  /// The last reading of the batch that decided it.
  #[serde(with = "time::serde::rfc3339")]
  pub at: OffsetDateTime,
  /// The pigeon whose batch decided it.
  pub pigeon_id: String,
  /// That pigeon's name.
  pub pigeon_name: String,
  /// The alert key.
  pub key: String,
  /// Fire, clear or notify.
  pub kind: RuleAlertKind,
  /// How the mail labels it.
  pub severity: RuleSeverity,
  /// The mail's subject, after the platform's prefix.
  pub subject: String,
  /// False when a mail cap would have held it back: the alert still moves.
  pub sent: bool,
}

/// The candidate's mails against the deployed version's, matched by pigeon, key and kind within
/// [`DRY_RUN_MAIL_MATCH_SECS`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleMailChange {
  /// Mails only the candidate sends, at most [`MAX_DRY_RUN_MAILS`].
  pub new: Vec<RuleDryMail>,
  /// Mails only the deployed version sends, at most [`MAX_DRY_RUN_MAILS`].
  pub gone: Vec<RuleDryMail>,
  /// Mails both send.
  pub unchanged: u32,
}

/// How one synthetic case went.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleCaseOutcome {
  /// The case's name.
  pub name: String,
  /// The mails sent match the mails expected, in any order, and no run failed.
  pub passed: bool,
  /// What the case expected.
  pub expected: Vec<RuleCaseMail>,
  /// What the candidate would have mailed.
  pub got: Vec<RuleCaseMail>,
  /// The error its failed runs gave most often.
  pub error: Option<String>,
}

/// One side of the CPU check.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleCpuResult {
  /// No batch was stopped. A first dispatch to a fresh script gets about 2 s of grace, so this is
  /// no promise the limit holds live.
  Pass,
  /// A batch lost its race or ran out of time under the live limit.
  Over,
  /// The script under the live limit did not load.
  DidNotLoad,
  /// Not run: nothing to replay, or the platform's API budget was spent.
  #[default]
  NotRun,
}

/// The CPU check: the candidate alone, then every enabled rule of the flock with the candidate in
/// its place, each as live dispatches under the live CPU limit over the replay's busiest batches.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleCpuCheck {
  /// The candidate alone.
  pub candidate: RuleCpuResult,
  /// The flock's enabled rules together, with the candidate.
  pub bundle: RuleCpuResult,
  /// Batches each side ran.
  pub batches: u32,
  /// Cloudflare's text when the script under the live limit did not load.
  pub load_error: Option<String>,
}

impl RuleCpuCheck {
  /// Whether the page warns. The check never refuses a deploy: a pass proves little on a fresh
  /// script, and the auto-pause stops a rule the limit kills live.
  pub fn warns(&self) -> bool {
    [self.candidate, self.bundle]
      .iter()
      .any(|r| matches!(r, RuleCpuResult::Over | RuleCpuResult::DidNotLoad))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn mail(at: i64, key: &str) -> RuleDryMail {
    RuleDryMail {
      at: OffsetDateTime::from_unix_timestamp(at).expect("valid time"),
      pigeon_id: "a".repeat(64),
      pigeon_name: "Sign a".into(),
      key: key.into(),
      kind: RuleAlertKind::Fire,
      severity: RuleSeverity::Warning,
      subject: "Hot".into(),
      sent: true,
    }
  }

  #[test]
  fn a_dry_run_round_trips_through_its_stored_json() {
    let replayed = RuleReplayed {
      version: 3,
      runs: 2,
      ok: 1,
      threw: 1,
      mails: vec![mail(1_800_000_000, "hot")],
      mails_total: 1,
      errors: vec![RuleErrorGroup {
        message: "TypeError: x".into(),
        count: 1,
      }],
      stopped_at: None,
      diverged_at: Some(OffsetDateTime::from_unix_timestamp(1_800_000_060).expect("valid")),
      ..RuleReplayed::default()
    };
    let run = RuleDryRun {
      window: RuleReplayWindow {
        from: Some(OffsetDateTime::from_unix_timestamp(1_799_990_000).expect("valid")),
        to: Some(OffsetDateTime::from_unix_timestamp(1_800_000_060).expect("valid")),
        readings: 2,
        batches: 2,
        pigeons: 1,
        cut: false,
      },
      candidate: replayed.clone(),
      deployed: None,
      seeded: None,
      mails: RuleMailChange {
        new: replayed.mails.clone(),
        gone: vec![],
        unchanged: 0,
      },
      cases: vec![],
      cpu: RuleCpuCheck::default(),
    };
    let text = serde_json::to_string(&run).expect("serializes");
    let back: RuleDryRun = serde_json::from_str(&text).expect("parses");
    assert_eq!(back, run);
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["candidate"]["mails"][0]["at"], "2027-01-15T08:00:00Z");
    assert_eq!(value["candidate"]["mails"][0]["kind"], "fire");
    assert_eq!(value["cpu"]["candidate"], "not_run");
  }

  #[test]
  fn the_cpu_check_warns_only_on_a_kill_or_a_load_failure() {
    let check = |candidate, bundle| RuleCpuCheck {
      candidate,
      bundle,
      batches: 10,
      load_error: None,
    };
    use RuleCpuResult::*;
    assert!(!check(Pass, Pass).warns());
    assert!(
      !check(NotRun, NotRun).warns(),
      "nothing run is nothing to warn of"
    );
    assert!(!check(Pass, NotRun).warns());
    assert!(check(Over, Pass).warns());
    assert!(
      check(Pass, Over).warns(),
      "the flock's rules together can exceed it"
    );
    assert!(check(DidNotLoad, NotRun).warns());
  }

  #[test]
  fn a_test_request_takes_no_body_fields_but_cases() {
    let empty: RuleTestRequest = serde_json::from_str("{}").expect("empty body");
    assert!(empty.cases.is_empty());
    let case = json!({
      "name": "just over the limit",
      "events": [{ "pigeon": "a", "readings": [
        { "after_secs": 0, "metrics": [{ "key": "temp_c", "value": "41" }] }
      ] }],
      "expect": [{ "key": "hot", "kind": "fire" }]
    });
    let req: RuleTestRequest =
      serde_json::from_value(json!({ "cases": [case.clone(), case] })).expect("cases");
    assert_eq!(req.cases.len(), 2);
    assert_eq!(req.cases[0].events[0].pigeon, RuleCasePigeon::A);
    assert!(serde_json::from_value::<RuleTestRequest>(json!({ "events": [] })).is_err());
  }

  #[test]
  fn five_cases_of_device_sized_batches_fit_the_body_cap() {
    // 14 pairs of the longest key and value are just under one device batch's raw bytes.
    let metrics: Vec<RuleCaseMetric> = (0..14u8)
      .map(|i| RuleCaseMetric {
        key: char::from(b'a' + i).to_string().repeat(128),
        value: "9".repeat(1024),
      })
      .collect();
    let event = RuleCaseEvent {
      pigeon: RuleCasePigeon::A,
      readings: vec![RuleCaseReading {
        after_secs: 604_800,
        metrics,
      }],
    };
    let case = RuleTestCase {
      name: "n".repeat(128),
      events: vec![event; 8],
      expect: vec![],
    };
    let body = serde_json::to_string(&RuleTestRequest {
      cases: vec![case; 5],
    })
    .expect("serializes");
    assert!(body.len() > 40 * 14 * 1152, "the body carries every batch");
    assert!(body.len() <= MAX_RULE_TEST_BODY_BYTES);
  }
}
