//! Rule contract 1: the event a user-authored rule receives and the result it may return.
//!
//! A rule runs as untrusted code in a Workers for Platforms user Worker. Its result reaches
//! dovecote as a JSON string, and [`validate_result`] is the only way in: it applies every cap of
//! the contract, counting UTF-8 bytes as the harness does, and refuses the whole result on the
//! first violation, so nothing of a bad result is ever applied. The JavaScript side of the same
//! contract is `dovecote/rules/contract.d.ts`; a change to either changes both.

use crate::{MAX_TELEMETRY_KEY_BYTES, MAX_TELEMETRY_VALUE_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The contract version a rule is written against, sent as the event's `contract`.
pub const RULE_CONTRACT: u32 = 1;

/// Largest serialized result one rule may return, in bytes. Its own cap because the per-field
/// caps below add up to about 70 KiB, and the response read cap is sized from this one.
pub const MAX_RULE_RESULT_BYTES: usize = 48 * 1024;

/// Most alerts in one result.
pub const MAX_RULE_ALERTS: usize = 8;

/// Longest alert key, in bytes of `[a-z0-9._:-]`.
pub const MAX_RULE_ALERT_KEY_BYTES: usize = 64;

/// Longest alert subject, in bytes. It becomes a mail subject, so no control character either.
pub const MAX_RULE_ALERT_SUBJECT_BYTES: usize = 120;

/// Longest alert body, in bytes. Newline is the only control character it may hold.
pub const MAX_RULE_ALERT_BODY_BYTES: usize = 1024;

/// Most derived values in one result. Each value's key and value take the telemetry caps,
/// [`MAX_TELEMETRY_KEY_BYTES`] and [`MAX_TELEMETRY_VALUE_BYTES`].
pub const MAX_RULE_VALUES: usize = 16;

/// Largest pigeon state document after a result's patch is merged, in bytes of compact JSON.
pub const MAX_RULE_PIGEON_STATE_BYTES: usize = 4 * 1024;

/// Largest flock state document after a result's patch is merged, in bytes of compact JSON.
pub const MAX_RULE_FLOCK_STATE_BYTES: usize = 32 * 1024;

/// Most state one dispatch may carry: every enabled rule's flock document plus this pigeon's.
/// Bounds what the fresh read moves out of Postgres per batch.
pub const MAX_RULE_STATE_TOTAL_BYTES: usize = 128 * 1024;

/// Most `console.log` lines kept from one rule run.
pub const MAX_RULE_LOG_LINES: usize = 20;

/// Longest kept `console.log` line, in bytes.
pub const MAX_RULE_LOG_LINE_BYTES: usize = 256;

/// Longest kept error text from a rule that threw, in bytes.
pub const MAX_RULE_ERROR_BYTES: usize = 1024;

/// Result fields refused as "not available yet", so a rule written now cannot start acting when
/// the seams they name open.
pub const RESERVED_RULE_RESULT_FIELDS: [&str; 4] = ["shadow", "fetch", "http", "webhook"];

/// What triggered a rule run. Later hooks extend this enum.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleHook {
  /// One telemetry batch of one pigeon.
  Telemetry,
}

/// The event one rule receives: one telemetry batch of one pigeon in the rule's flock.
///
/// Built from named fields only, so nothing a rule must not see (connector, endpoint token,
/// owner) can ride along.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleEvent {
  /// Always [`RULE_CONTRACT`].
  pub contract: u32,
  /// The trigger; only [`RuleHook::Telemetry`] exists in contract 1.
  pub hook: RuleHook,
  /// 64 lowercase hex, equal on a redelivery of the same batch.
  pub batch_id: String,
  /// Unix seconds when dovecote built the event; in a dry run, the batch's last reading.
  pub received_at: i64,
  /// True when the batch's `previous` values were lost; every reading's `previous` is then `{}`.
  pub degraded: bool,
  /// The rule being run.
  pub rule: RuleVersionRef,
  /// The flock the rule belongs to.
  pub flock: RuleNamedRef,
  /// The pigeon whose batch this is.
  pub pigeon: RuleNamedRef,
  /// 1 to [`crate::MAX_TELEMETRY_BATCH_READINGS`] readings, oldest first.
  pub readings: Vec<RuleReading>,
  /// What this rule has stored for this pigeon and for its flock.
  pub state: RuleState,
}

/// The rule being run, at the version that runs.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuleVersionRef {
  /// The rule's id.
  pub id: String,
  /// The deployed version that runs.
  pub version: i32,
}

/// A flock or pigeon as a rule sees it: id and name, nothing else.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuleNamedRef {
  /// The flock's or pigeon's id.
  pub id: String,
  /// Its display name.
  pub name: String,
}

/// One reading of one pigeon, in the contract's spelling.
///
/// Not the pipeline's `ResolvedReading`, whose `at_secs`, optional `previous` and `reported_at`
/// would break every rule; `previous` here is always present, `{}` when nothing was displaced.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuleReading {
  /// Unix seconds, already clamped by the server.
  pub at: i64,
  /// Key to value as the device sent it; values are always strings.
  pub metrics: BTreeMap<String, String>,
  /// Per key, the stored entry this reading displaced.
  pub previous: BTreeMap<String, RulePrevious>,
}

/// The stored entry a reading displaced.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RulePrevious {
  /// The displaced value, a string like every telemetry value.
  pub value: String,
  /// Unix seconds.
  pub at: i64,
}

/// One rule's two state documents: this pigeon's and the flock's, `{}` when empty.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct RuleState {
  /// This rule's document for this pigeon.
  pub pigeon: Map<String, Value>,
  /// This rule's document for the whole flock, shared by every pigeon's runs.
  pub flock: Map<String, Value>,
}

/// A validated rule result. Only [`validate_result`] builds one from a rule's output.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleResult {
  /// Up to [`MAX_RULE_ALERTS`] mail decisions.
  pub alerts: Vec<RuleAlert>,
  /// Up to [`MAX_RULE_VALUES`] derived values.
  pub values: Vec<RuleResultValue>,
  /// The patches to this rule's two state documents.
  pub state: RuleStatePatch,
}

/// A mail decision. The rule picks the words; the rule's settings pick the recipients.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleAlert {
  /// 1 to [`MAX_RULE_ALERT_KEY_BYTES`] bytes of `[a-z0-9._:-]`.
  pub key: String,
  /// When it mails.
  pub kind: RuleAlertKind,
  /// How the mail labels it.
  pub severity: RuleSeverity,
  /// Whether `key` names a per-pigeon or a flock-wide condition.
  pub scope: RuleAlertScope,
  /// 1 to [`MAX_RULE_ALERT_SUBJECT_BYTES`] bytes, no control character.
  pub subject: String,
  /// Up to [`MAX_RULE_ALERT_BODY_BYTES`] bytes; newline the only control character.
  pub body: Option<String>,
}

/// When an alert mails.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleAlertKind {
  /// Once when the condition starts holding.
  Fire,
  /// Once when it stops, only after a fire.
  Clear,
  /// Once per key within 7 days.
  Notify,
}

/// An alert's severity as the mail shows it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleSeverity {
  /// For the record; nothing is wrong.
  Info,
  /// Worth a look.
  Warning,
  /// Needs action now.
  Critical,
}

/// Whose condition an alert key names.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleAlertScope {
  /// One state per pigeon.
  Pigeon,
  /// One state for the whole flock, so the fleet gets one mail.
  Flock,
}

/// A derived value: the contract's `RuleValue`, kept beside telemetry and never mixed into it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleResultValue {
  /// 1 to [`MAX_TELEMETRY_KEY_BYTES`] bytes.
  pub key: String,
  /// Up to [`MAX_TELEMETRY_VALUE_BYTES`] bytes.
  pub value: String,
  /// Unix seconds as the rule gave it; the caller clamps it into the batch, `None` meaning the
  /// last reading.
  pub at: Option<i64>,
}

/// A shallow patch per state document, applied by [`merge_state`]; `None` leaves it untouched.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleStatePatch {
  /// The patch to this pigeon's document; `null` values delete keys.
  pub pigeon: Option<Map<String, Value>>,
  /// The patch to the flock's document; `null` values delete keys.
  pub flock: Option<Map<String, Value>>,
}

/// The first contract violation in a refused result, recorded as the run's `bad_output`.
///
/// Its text names a place and a problem only, never the rule's own text, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleViolation {
  /// Where: `result`, a reserved field's name, or a path such as `alerts.subject`.
  pub field: &'static str,
  /// The item of `alerts` or `values` at fault, when the violation is inside one.
  pub index: Option<usize>,
  /// What was wrong there.
  pub problem: RuleProblem,
}

/// What was wrong at a [`RuleViolation`]'s place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleProblem {
  /// The result, or a state document after the merge, is over its byte cap.
  TooLarge,
  /// The result does not parse, or nests deeper than the parser allows.
  NotJson,
  /// The result is JSON but not an object.
  NotAnObject,
  /// A field from [`RESERVED_RULE_RESULT_FIELDS`].
  NotAvailableYet,
  /// A field the contract does not name.
  UnknownField,
  /// A field of the wrong JSON type, including `null` where the contract has none.
  WrongType,
  /// A required field is absent.
  Missing,
  /// More alerts or values than the cap.
  TooMany,
  /// A string that must not be empty is.
  Empty,
  /// A string over its byte cap.
  TooLong,
  /// An alert key outside `[a-z0-9._:-]`.
  BadCharacter,
  /// A control character in a subject, or one other than newline in a body.
  ControlCharacter,
  /// A word outside an enum's spellings.
  BadValue,
}

/// Validates one rule's serialized result against contract 1 and the rule's state snapshot.
///
/// All or nothing: the first violation refuses the whole result. Every byte cap counts UTF-8
/// bytes. The state caps apply to each document after `state`'s patch is merged into the
/// snapshot the rule was given, and only to a document the result patches; a concurrent writer
/// can still overshoot by one document, which the caller's state budget absorbs.
pub fn validate_result(out_json: &str, state: &RuleState) -> Result<RuleResult, RuleViolation> {
  if out_json.len() > MAX_RULE_RESULT_BYTES {
    return Err(at("result", RuleProblem::TooLarge));
  }
  let Ok(parsed) = serde_json::from_str::<Value>(out_json) else {
    return Err(at("result", RuleProblem::NotJson));
  };
  let Value::Object(top) = parsed else {
    return Err(at("result", RuleProblem::NotAnObject));
  };
  if let Some(name) = RESERVED_RULE_RESULT_FIELDS
    .into_iter()
    .find(|n| top.contains_key(*n))
  {
    return Err(at(name, RuleProblem::NotAvailableYet));
  }
  only(&top, &["alerts", "values", "state"], "result", None)?;

  let mut result = RuleResult::default();
  if let Some(alerts) = top.get("alerts") {
    for (i, item) in list(alerts, "alerts", MAX_RULE_ALERTS)?.iter().enumerate() {
      result.alerts.push(alert(item, i)?);
    }
  }
  if let Some(values) = top.get("values") {
    for (i, item) in list(values, "values", MAX_RULE_VALUES)?.iter().enumerate() {
      result.values.push(derived_value(item, i)?);
    }
  }
  if let Some(patch) = top.get("state") {
    let Value::Object(patch) = patch else {
      return Err(at("state", RuleProblem::WrongType));
    };
    only(patch, &["pigeon", "flock"], "state", None)?;
    result.state.pigeon = document(patch, "pigeon", &state.pigeon)?;
    result.state.flock = document(patch, "flock", &state.flock)?;
  }
  Ok(result)
}

/// Applies a shallow patch: each top-level key is set, `null` deletes it, an absent key is kept.
pub fn merge_state(stored: &Map<String, Value>, patch: &Map<String, Value>) -> Map<String, Value> {
  let mut merged = stored.clone();
  for (key, value) in patch {
    if value.is_null() {
      merged.remove(key);
    } else {
      merged.insert(key.clone(), value.clone());
    }
  }
  merged
}

/// A state document's size as the caps count it: bytes of its compact JSON.
pub fn state_doc_bytes(doc: &Map<String, Value>) -> usize {
  serde_json::to_vec(doc).map_or(usize::MAX, |b| b.len())
}

impl std::fmt::Display for RuleViolation {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match (self.index, self.field.split_once('.')) {
      (Some(i), Some((list, field))) => write!(f, "{list}[{i}].{field}")?,
      (Some(i), None) => write!(f, "{}[{i}]", self.field)?,
      (None, _) => f.write_str(self.field)?,
    }
    f.write_str(": ")?;
    f.write_str(self.problem.as_str())
  }
}

impl RuleProblem {
  /// Fixed text for the problem, as a `bad_output` record shows it.
  pub fn as_str(&self) -> &'static str {
    match self {
      RuleProblem::TooLarge => "too large",
      RuleProblem::NotJson => "not JSON",
      RuleProblem::NotAnObject => "not an object",
      RuleProblem::NotAvailableYet => "not available yet",
      RuleProblem::UnknownField => "unknown field",
      RuleProblem::WrongType => "wrong type",
      RuleProblem::Missing => "missing",
      RuleProblem::TooMany => "too many",
      RuleProblem::Empty => "empty",
      RuleProblem::TooLong => "too long",
      RuleProblem::BadCharacter => "character outside [a-z0-9._:-]",
      RuleProblem::ControlCharacter => "control character",
      RuleProblem::BadValue => "not an allowed value",
    }
  }
}

fn at(field: &'static str, problem: RuleProblem) -> RuleViolation {
  RuleViolation {
    field,
    index: None,
    problem,
  }
}

fn item(field: &'static str, index: usize, problem: RuleProblem) -> RuleViolation {
  RuleViolation {
    field,
    index: Some(index),
    problem,
  }
}

/// Refuses any key of `obj` outside `allowed`.
fn only(
  obj: &Map<String, Value>,
  allowed: &[&str],
  field: &'static str,
  index: Option<usize>,
) -> Result<(), RuleViolation> {
  if obj.keys().all(|k| allowed.contains(&k.as_str())) {
    return Ok(());
  }
  Err(RuleViolation {
    field,
    index,
    problem: RuleProblem::UnknownField,
  })
}

fn list<'a>(v: &'a Value, field: &'static str, max: usize) -> Result<&'a [Value], RuleViolation> {
  let Value::Array(items) = v else {
    return Err(at(field, RuleProblem::WrongType));
  };
  if items.len() > max {
    return Err(at(field, RuleProblem::TooMany));
  }
  Ok(items)
}

/// The string at `name`: `Ok(None)` when absent, refused when present and not a string.
fn text<'a>(
  obj: &'a Map<String, Value>,
  name: &str,
  field: &'static str,
  i: usize,
) -> Result<Option<&'a str>, RuleViolation> {
  match obj.get(name) {
    None => Ok(None),
    Some(Value::String(s)) => Ok(Some(s)),
    Some(_) => Err(item(field, i, RuleProblem::WrongType)),
  }
}

fn required<'a>(
  obj: &'a Map<String, Value>,
  name: &str,
  field: &'static str,
  i: usize,
) -> Result<&'a str, RuleViolation> {
  text(obj, name, field, i)?.ok_or(item(field, i, RuleProblem::Missing))
}

/// Applies a byte range and the empty rule to one string field.
fn sized(
  s: &str,
  min: usize,
  max: usize,
  field: &'static str,
  i: usize,
) -> Result<(), RuleViolation> {
  if s.len() < min {
    return Err(item(field, i, RuleProblem::Empty));
  }
  if s.len() > max {
    return Err(item(field, i, RuleProblem::TooLong));
  }
  Ok(())
}

fn alert(v: &Value, i: usize) -> Result<RuleAlert, RuleViolation> {
  let Value::Object(obj) = v else {
    return Err(item("alerts", i, RuleProblem::WrongType));
  };
  let names = ["key", "kind", "severity", "scope", "subject", "body"];
  only(obj, &names, "alerts", Some(i))?;

  let key = required(obj, "key", "alerts.key", i)?;
  sized(key, 1, MAX_RULE_ALERT_KEY_BYTES, "alerts.key", i)?;
  let alphabet = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:_-".contains(&b);
  if !key.bytes().all(alphabet) {
    return Err(item("alerts.key", i, RuleProblem::BadCharacter));
  }
  let kind = match required(obj, "kind", "alerts.kind", i)? {
    "fire" => RuleAlertKind::Fire,
    "clear" => RuleAlertKind::Clear,
    "notify" => RuleAlertKind::Notify,
    _ => return Err(item("alerts.kind", i, RuleProblem::BadValue)),
  };
  let severity = match required(obj, "severity", "alerts.severity", i)? {
    "info" => RuleSeverity::Info,
    "warning" => RuleSeverity::Warning,
    "critical" => RuleSeverity::Critical,
    _ => return Err(item("alerts.severity", i, RuleProblem::BadValue)),
  };
  let scope = match required(obj, "scope", "alerts.scope", i)? {
    "pigeon" => RuleAlertScope::Pigeon,
    "flock" => RuleAlertScope::Flock,
    _ => return Err(item("alerts.scope", i, RuleProblem::BadValue)),
  };
  // A subject becomes a mail header, so CR and LF are as forbidden as the rest.
  let subject = required(obj, "subject", "alerts.subject", i)?;
  sized(
    subject,
    1,
    MAX_RULE_ALERT_SUBJECT_BYTES,
    "alerts.subject",
    i,
  )?;
  if subject.chars().any(char::is_control) {
    return Err(item("alerts.subject", i, RuleProblem::ControlCharacter));
  }
  let body = text(obj, "body", "alerts.body", i)?;
  if let Some(body) = body {
    sized(body, 0, MAX_RULE_ALERT_BODY_BYTES, "alerts.body", i)?;
    if body.chars().any(|c| c.is_control() && c != '\n') {
      return Err(item("alerts.body", i, RuleProblem::ControlCharacter));
    }
  }
  Ok(RuleAlert {
    key: key.to_string(),
    kind,
    severity,
    scope,
    subject: subject.to_string(),
    body: body.map(str::to_string),
  })
}

fn derived_value(v: &Value, i: usize) -> Result<RuleResultValue, RuleViolation> {
  let Value::Object(obj) = v else {
    return Err(item("values", i, RuleProblem::WrongType));
  };
  only(obj, &["key", "value", "at"], "values", Some(i))?;
  let key = required(obj, "key", "values.key", i)?;
  sized(key, 1, MAX_TELEMETRY_KEY_BYTES, "values.key", i)?;
  let value = required(obj, "value", "values.value", i)?;
  sized(value, 0, MAX_TELEMETRY_VALUE_BYTES, "values.value", i)?;
  let at = match obj.get("at") {
    None => None,
    Some(n) => Some(
      n.as_i64()
        .ok_or(item("values.at", i, RuleProblem::WrongType))?,
    ),
  };
  Ok(RuleResultValue {
    key: key.to_string(),
    value: value.to_string(),
    at,
  })
}

/// One document's patch, checked against its cap once merged into the stored copy.
fn document(
  patch: &Map<String, Value>,
  name: &str,
  stored: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>, RuleViolation> {
  let (field, max) = if name == "pigeon" {
    ("state.pigeon", MAX_RULE_PIGEON_STATE_BYTES)
  } else {
    ("state.flock", MAX_RULE_FLOCK_STATE_BYTES)
  };
  let Some(doc) = patch.get(name) else {
    return Ok(None);
  };
  let Value::Object(doc) = doc else {
    return Err(at(field, RuleProblem::WrongType));
  };
  if state_doc_bytes(&merge_state(stored, doc)) > max {
    return Err(at(field, RuleProblem::TooLarge));
  }
  Ok(Some(doc.clone()))
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn empty_state() -> RuleState {
    RuleState::default()
  }

  fn check(out: serde_json::Value) -> Result<RuleResult, RuleViolation> {
    validate_result(&out.to_string(), &empty_state())
  }

  fn refused(out: serde_json::Value) -> RuleViolation {
    check(out).expect_err("result should be refused")
  }

  fn alert() -> serde_json::Value {
    json!({
      "key": "reset.watchdog:17",
      "kind": "notify",
      "severity": "warning",
      "scope": "pigeon",
      "subject": "Watchdog reset",
      "body": "stage: render\nuptime 42 s"
    })
  }

  fn value() -> serde_json::Value {
    json!({ "key": "boots_24h", "value": "4", "at": 1_767_225_600 })
  }

  fn with_alert(field: &str, v: serde_json::Value) -> serde_json::Value {
    let mut a = alert();
    a[field] = v;
    json!({ "alerts": [a] })
  }

  fn with_value(field: &str, v: serde_json::Value) -> serde_json::Value {
    let mut a = value();
    a[field] = v;
    json!({ "values": [a] })
  }

  fn is(v: &RuleViolation, field: &str, index: Option<usize>, problem: RuleProblem) {
    assert_eq!(
      (v.field, v.index, v.problem),
      (field, index, problem),
      "got {v}"
    );
  }

  // The contract's `Reading`: `at`, `metrics`, and `previous` as `{value, at}` per key, `{}` when
  // nothing was displaced. Never the pipeline's `at_secs` or `reported_at`.
  #[test]
  fn rule_reading_json_is_pinned_to_the_contract() {
    let reading = RuleReading {
      at: 1_767_225_600,
      metrics: [("reset_cause", "17"), ("uptime_s", "42")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
      previous: [(
        "uptime_s".to_string(),
        RulePrevious {
          value: "86000".to_string(),
          at: 1_767_225_500,
        },
      )]
      .into_iter()
      .collect(),
    };
    assert_eq!(
      serde_json::to_string(&reading).unwrap(),
      r#"{"at":1767225600,"metrics":{"reset_cause":"17","uptime_s":"42"},"previous":{"uptime_s":{"value":"86000","at":1767225500}}}"#
    );
  }

  #[test]
  fn rule_reading_with_nothing_displaced_serializes_previous_as_an_empty_object() {
    let reading = RuleReading {
      at: 5,
      metrics: BTreeMap::new(),
      previous: BTreeMap::new(),
    };
    assert_eq!(
      serde_json::to_string(&reading).unwrap(),
      r#"{"at":5,"metrics":{},"previous":{}}"#
    );
  }

  #[test]
  fn rule_reading_refuses_the_pipelines_field_names() {
    let at_secs = r#"{"at_secs":5,"metrics":{},"previous":{}}"#;
    assert!(serde_json::from_str::<RuleReading>(at_secs).is_err());
    let reported_at = r#"{"at":5,"metrics":{},"previous":{"k":{"value":"1","reported_at":4}}}"#;
    assert!(serde_json::from_str::<RuleReading>(reported_at).is_err());
  }

  #[test]
  fn rule_event_json_matches_the_contract() {
    let event = RuleEvent {
      contract: RULE_CONTRACT,
      hook: RuleHook::Telemetry,
      batch_id: "ab".repeat(32),
      received_at: 10,
      degraded: true,
      rule: RuleVersionRef {
        id: "r1".to_string(),
        version: 3,
      },
      flock: RuleNamedRef {
        id: "f1".to_string(),
        name: "Signs".to_string(),
      },
      pigeon: RuleNamedRef {
        id: "p1".to_string(),
        name: "414".to_string(),
      },
      readings: vec![RuleReading {
        at: 9,
        metrics: BTreeMap::new(),
        previous: BTreeMap::new(),
      }],
      state: RuleState::default(),
    };
    let expected = json!({
      "contract": 1,
      "hook": "telemetry",
      "batch_id": "ab".repeat(32),
      "received_at": 10,
      "degraded": true,
      "rule": { "id": "r1", "version": 3 },
      "flock": { "id": "f1", "name": "Signs" },
      "pigeon": { "id": "p1", "name": "414" },
      "readings": [{ "at": 9, "metrics": {}, "previous": {} }],
      "state": { "pigeon": {}, "flock": {} }
    });
    assert_eq!(serde_json::to_value(&event).unwrap(), expected);
  }

  #[test]
  fn an_empty_object_is_a_quiet_result() {
    let r = check(json!({})).unwrap();
    assert!(r.alerts.is_empty() && r.values.is_empty());
    assert_eq!(r.state, RuleStatePatch::default());
  }

  #[test]
  fn a_full_result_within_every_cap_is_accepted() {
    let alerts: Vec<_> = (0..MAX_RULE_ALERTS)
      .map(|i| {
        let mut a = alert();
        a["key"] = json!(format!("k{i}"));
        a
      })
      .collect();
    let values: Vec<_> = (0..MAX_RULE_VALUES).map(|_| value()).collect();
    let out = json!({
      "alerts": alerts,
      "values": values,
      "state": { "pigeon": { "boots": [1, 2] }, "flock": { "down": true } }
    });
    let r = check(out).unwrap();
    assert_eq!(r.alerts.len(), MAX_RULE_ALERTS);
    assert_eq!(r.values.len(), MAX_RULE_VALUES);
    assert_eq!(r.alerts[0].kind, RuleAlertKind::Notify);
    assert_eq!(r.alerts[0].severity, RuleSeverity::Warning);
    assert_eq!(r.alerts[0].scope, RuleAlertScope::Pigeon);
    assert_eq!(
      r.alerts[0].body.as_deref(),
      Some("stage: render\nuptime 42 s")
    );
    assert_eq!(r.values[0].at, Some(1_767_225_600));
    assert_eq!(r.state.pigeon.unwrap()["boots"], json!([1, 2]));
  }

  #[test]
  fn every_enum_spelling_is_accepted() {
    for (field, word) in [
      ("kind", "fire"),
      ("kind", "clear"),
      ("kind", "notify"),
      ("severity", "info"),
      ("severity", "warning"),
      ("severity", "critical"),
      ("scope", "pigeon"),
      ("scope", "flock"),
    ] {
      assert!(
        check(with_alert(field, json!(word))).is_ok(),
        "{field} {word}"
      );
    }
  }

  #[test]
  fn result_bytes_cap_counts_utf8_bytes() {
    // "é" is two UTF-8 bytes and one UTF-16 unit, so this is under the cap in units, over it in
    // bytes.
    let pad = "é".repeat(MAX_RULE_RESULT_BYTES / 2);
    let out = json!({ "state": { "flock": { "p": pad } } }).to_string();
    assert!(out.encode_utf16().count() <= MAX_RULE_RESULT_BYTES);
    let v = validate_result(&out, &empty_state()).unwrap_err();
    is(&v, "result", None, RuleProblem::TooLarge);
  }

  #[test]
  fn result_at_exactly_the_cap_is_measured_not_refused() {
    let base = r#"{"values":[]}"#;
    let out = base.to_string() + &" ".repeat(MAX_RULE_RESULT_BYTES - base.len());
    assert_eq!(out.len(), MAX_RULE_RESULT_BYTES);
    assert!(validate_result(&out, &empty_state()).is_ok());
    let over = out + " ";
    let v = validate_result(&over, &empty_state()).unwrap_err();
    is(&v, "result", None, RuleProblem::TooLarge);
  }

  #[test]
  fn not_json_and_not_an_object_are_refused() {
    let v = validate_result("{", &empty_state()).unwrap_err();
    is(&v, "result", None, RuleProblem::NotJson);
    for out in ["[]", "null", "1", "\"x\""] {
      let v = validate_result(out, &empty_state()).unwrap_err();
      is(&v, "result", None, RuleProblem::NotAnObject);
    }
  }

  #[test]
  fn reserved_fields_answer_not_available_yet() {
    for name in RESERVED_RULE_RESULT_FIELDS {
      let mut out = json!({});
      out[name] = json!({ "anything": 1 });
      let v = refused(out);
      is(&v, name, None, RuleProblem::NotAvailableYet);
      assert_eq!(v.to_string(), name.to_string() + ": not available yet");
    }
  }

  #[test]
  fn shadow_is_refused_even_beside_a_good_result() {
    let v = refused(json!({ "alerts": [alert()], "shadow": { "log": "on" } }));
    is(&v, "shadow", None, RuleProblem::NotAvailableYet);
  }

  #[test]
  fn unknown_fields_are_refused_at_every_level() {
    is(
      &refused(json!({ "extra": 1 })),
      "result",
      None,
      RuleProblem::UnknownField,
    );
    let mut a = alert();
    a["recipients"] = json!(["a@example.com"]);
    is(
      &refused(json!({ "alerts": [a] })),
      "alerts",
      Some(0),
      RuleProblem::UnknownField,
    );
    let mut v = value();
    v["unit"] = json!("V");
    is(
      &refused(json!({ "values": [v] })),
      "values",
      Some(0),
      RuleProblem::UnknownField,
    );
    is(
      &refused(json!({ "state": { "device": {} } })),
      "state",
      None,
      RuleProblem::UnknownField,
    );
  }

  #[test]
  fn wrong_types_are_refused() {
    is(
      &refused(json!({ "alerts": {} })),
      "alerts",
      None,
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "alerts": null })),
      "alerts",
      None,
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "alerts": [1] })),
      "alerts",
      Some(0),
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "values": "x" })),
      "values",
      None,
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "values": [[]] })),
      "values",
      Some(0),
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "state": [] })),
      "state",
      None,
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "state": { "pigeon": null } })),
      "state.pigeon",
      None,
      RuleProblem::WrongType,
    );
    is(
      &refused(json!({ "state": { "flock": "x" } })),
      "state.flock",
      None,
      RuleProblem::WrongType,
    );
    for field in ["key", "kind", "severity", "scope", "subject", "body"] {
      let v = refused(with_alert(field, json!(7)));
      assert_eq!(
        (v.index, v.problem),
        (Some(0), RuleProblem::WrongType),
        "{field}"
      );
      assert_eq!(v.field.strip_prefix("alerts."), Some(field));
    }
    is(
      &refused(with_alert("body", json!(null))),
      "alerts.body",
      Some(0),
      RuleProblem::WrongType,
    );
    is(
      &refused(with_value("key", json!(1))),
      "values.key",
      Some(0),
      RuleProblem::WrongType,
    );
    is(
      &refused(with_value("value", json!(4))),
      "values.value",
      Some(0),
      RuleProblem::WrongType,
    );
    is(
      &refused(with_value("at", json!("9"))),
      "values.at",
      Some(0),
      RuleProblem::WrongType,
    );
    is(
      &refused(with_value("at", json!(1.5))),
      "values.at",
      Some(0),
      RuleProblem::WrongType,
    );
  }

  #[test]
  fn required_alert_and_value_fields_are_refused_when_missing() {
    for field in ["key", "kind", "severity", "scope", "subject"] {
      let mut a = alert();
      a.as_object_mut().unwrap().remove(field);
      let v = refused(json!({ "alerts": [a] }));
      assert_eq!(
        (v.index, v.problem),
        (Some(0), RuleProblem::Missing),
        "{field}"
      );
      assert_eq!(v.field.strip_prefix("alerts."), Some(field));
    }
    for field in ["key", "value"] {
      let mut a = value();
      a.as_object_mut().unwrap().remove(field);
      let v = refused(json!({ "values": [a] }));
      assert_eq!(
        (v.index, v.problem),
        (Some(0), RuleProblem::Missing),
        "{field}"
      );
      assert_eq!(v.field.strip_prefix("values."), Some(field));
    }
    let mut a = alert();
    a.as_object_mut().unwrap().remove("body");
    let r = check(json!({ "alerts": [a] })).unwrap();
    assert_eq!(r.alerts[0].body, None);
    let mut v = value();
    v.as_object_mut().unwrap().remove("at");
    assert_eq!(check(json!({ "values": [v] })).unwrap().values[0].at, None);
  }

  #[test]
  fn enum_fields_refuse_other_words() {
    for field in ["kind", "severity", "scope"] {
      let v = refused(with_alert(field, json!("Fire")));
      assert_eq!(
        (v.index, v.problem),
        (Some(0), RuleProblem::BadValue),
        "{field}"
      );
    }
  }

  #[test]
  fn alert_count_cap() {
    let alerts: Vec<_> = (0..=MAX_RULE_ALERTS).map(|_| alert()).collect();
    is(
      &refused(json!({ "alerts": alerts })),
      "alerts",
      None,
      RuleProblem::TooMany,
    );
  }

  #[test]
  fn value_count_cap() {
    let values: Vec<_> = (0..=MAX_RULE_VALUES).map(|_| value()).collect();
    is(
      &refused(json!({ "values": values })),
      "values",
      None,
      RuleProblem::TooMany,
    );
  }

  #[test]
  fn alert_key_cap_and_alphabet() {
    let at_cap = "a".repeat(MAX_RULE_ALERT_KEY_BYTES);
    assert!(check(with_alert("key", json!(at_cap))).is_ok());
    let over = "a".repeat(MAX_RULE_ALERT_KEY_BYTES + 1);
    is(
      &refused(with_alert("key", json!(over))),
      "alerts.key",
      Some(0),
      RuleProblem::TooLong,
    );
    is(
      &refused(with_alert("key", json!(""))),
      "alerts.key",
      Some(0),
      RuleProblem::Empty,
    );
    for bad in ["Upper", "sp ace", "slash/", "é", "new\nline"] {
      let v = refused(with_alert("key", json!(bad)));
      is(&v, "alerts.key", Some(0), RuleProblem::BadCharacter);
    }
    assert!(check(with_alert("key", json!("a-z.0_9:x"))).is_ok());
  }

  #[test]
  fn subject_cap_counts_utf8_bytes() {
    let at_cap = "é".repeat(MAX_RULE_ALERT_SUBJECT_BYTES / 2);
    assert!(check(with_alert("subject", json!(at_cap))).is_ok());
    // 61 characters, 122 bytes: under the cap in characters, over it in bytes.
    let over = "é".repeat(MAX_RULE_ALERT_SUBJECT_BYTES / 2 + 1);
    let v = refused(with_alert("subject", json!(over)));
    is(&v, "alerts.subject", Some(0), RuleProblem::TooLong);
    let v = refused(with_alert("subject", json!("")));
    is(&v, "alerts.subject", Some(0), RuleProblem::Empty);
  }

  #[test]
  fn subject_refuses_every_control_character() {
    for bad in [
      "a\nb",
      "a\rb",
      "a\tb",
      "a\u{0}b",
      "a\u{7f}b",
      "a\u{85}b",
      "a\u{1b}[31m",
    ] {
      let v = refused(with_alert("subject", json!(bad)));
      is(&v, "alerts.subject", Some(0), RuleProblem::ControlCharacter);
    }
  }

  #[test]
  fn body_cap_counts_utf8_bytes() {
    let at_cap = "é".repeat(MAX_RULE_ALERT_BODY_BYTES / 2);
    assert!(check(with_alert("body", json!(at_cap))).is_ok());
    let over = "é".repeat(MAX_RULE_ALERT_BODY_BYTES / 2) + "x";
    let v = refused(with_alert("body", json!(over)));
    is(&v, "alerts.body", Some(0), RuleProblem::TooLong);
    assert!(check(with_alert("body", json!(""))).is_ok());
  }

  #[test]
  fn body_allows_newline_and_no_other_control_character() {
    assert!(check(with_alert("body", json!("line one\nline two\n"))).is_ok());
    for bad in ["a\r\nb", "a\tb", "a\u{0}b", "a\u{7f}b", "a\u{9b}b"] {
      let v = refused(with_alert("body", json!(bad)));
      is(&v, "alerts.body", Some(0), RuleProblem::ControlCharacter);
    }
  }

  #[test]
  fn value_key_and_value_caps_count_utf8_bytes() {
    assert!(
      check(with_value(
        "key",
        json!("k".repeat(MAX_TELEMETRY_KEY_BYTES))
      ))
      .is_ok()
    );
    let over = "é".repeat(MAX_TELEMETRY_KEY_BYTES / 2) + "x";
    is(
      &refused(with_value("key", json!(over))),
      "values.key",
      Some(0),
      RuleProblem::TooLong,
    );
    is(
      &refused(with_value("key", json!(""))),
      "values.key",
      Some(0),
      RuleProblem::Empty,
    );
    assert!(
      check(with_value(
        "value",
        json!("v".repeat(MAX_TELEMETRY_VALUE_BYTES))
      ))
      .is_ok()
    );
    assert!(check(with_value("value", json!(""))).is_ok());
    let over = "é".repeat(MAX_TELEMETRY_VALUE_BYTES / 2) + "x";
    let v = refused(with_value("value", json!(over)));
    is(&v, "values.value", Some(0), RuleProblem::TooLong);
  }

  #[test]
  fn violations_name_the_offending_index() {
    let mut bad = alert();
    bad["subject"] = json!("x\ry");
    let v = refused(json!({ "alerts": [alert(), alert(), bad] }));
    is(&v, "alerts.subject", Some(2), RuleProblem::ControlCharacter);
    assert_eq!(v.to_string(), "alerts[2].subject: control character");
  }

  #[test]
  fn violation_text_never_carries_the_rules_own_text() {
    let secret = "jane@example.com";
    let mut a = alert();
    a[secret] = json!(secret);
    let v = refused(json!({ "alerts": [a] }));
    assert!(!v.to_string().contains("jane"), "{v}");
    let v = refused(with_alert("subject", json!(secret.to_string() + "\n")));
    assert!(!v.to_string().contains("jane"), "{v}");
  }

  #[test]
  fn one_bad_part_refuses_the_whole_result() {
    let mut bad_value = value();
    bad_value["value"] = json!("v".repeat(MAX_TELEMETRY_VALUE_BYTES + 1));
    let out = json!({
      "alerts": [alert()],
      "values": [value(), bad_value],
      "state": { "pigeon": { "ok": 1 } }
    });
    let v = refused(out);
    is(&v, "values.value", Some(1), RuleProblem::TooLong);

    let out = json!({
      "alerts": [alert()],
      "values": [value()],
      "state": { "flock": { "big": "x".repeat(MAX_RULE_FLOCK_STATE_BYTES) } }
    });
    is(&refused(out), "state.flock", None, RuleProblem::TooLarge);
  }

  #[test]
  fn state_patch_sets_deletes_and_keeps() {
    let mut stored = serde_json::Map::new();
    stored.insert("keep".into(), json!(1));
    stored.insert("drop".into(), json!(2));
    stored.insert("swap".into(), json!(3));
    let patch = json!({ "drop": null, "swap": "new", "add": [1] });
    let merged = merge_state(&stored, patch.as_object().unwrap());
    assert_eq!(
      serde_json::Value::Object(merged),
      json!({ "keep": 1, "swap": "new", "add": [1] })
    );
  }

  #[test]
  fn pigeon_state_cap_applies_after_the_merge() {
    // Stored document near its cap: a small addition overflows, a deletion beside it fits.
    let mut state = RuleState::default();
    let filler = "x".repeat(MAX_RULE_PIGEON_STATE_BYTES - 32);
    state.pigeon.insert("big".into(), json!(filler));
    assert!(state_doc_bytes(&state.pigeon) <= MAX_RULE_PIGEON_STATE_BYTES);

    let grow = json!({ "state": { "pigeon": { "more": "y".repeat(40) } } }).to_string();
    let v = validate_result(&grow, &state).unwrap_err();
    is(&v, "state.pigeon", None, RuleProblem::TooLarge);

    let shrink =
      json!({ "state": { "pigeon": { "big": null, "more": "y".repeat(40) } } }).to_string();
    assert!(validate_result(&shrink, &state).is_ok());
  }

  #[test]
  fn state_caps_are_exact_in_utf8_bytes() {
    // {"k":"..."} is 8 bytes of framing around the string.
    let fits = "x".repeat(MAX_RULE_PIGEON_STATE_BYTES - 8);
    let out = json!({ "state": { "pigeon": { "k": fits } } }).to_string();
    assert!(validate_result(&out, &empty_state()).is_ok());
    let over = "é".repeat((MAX_RULE_PIGEON_STATE_BYTES - 8) / 2) + "x";
    let out = json!({ "state": { "pigeon": { "k": over } } }).to_string();
    let v = validate_result(&out, &empty_state()).unwrap_err();
    is(&v, "state.pigeon", None, RuleProblem::TooLarge);

    let fits = "x".repeat(MAX_RULE_FLOCK_STATE_BYTES - 8);
    let mut state = RuleState::default();
    state.flock.insert("k".into(), json!(fits));
    let out = json!({ "state": { "flock": {} } }).to_string();
    assert!(validate_result(&out, &state).is_ok());
    let out = json!({ "state": { "flock": { "j": 1 } } }).to_string();
    let v = validate_result(&out, &state).unwrap_err();
    is(&v, "state.flock", None, RuleProblem::TooLarge);
  }

  #[test]
  fn an_oversized_stored_document_left_untouched_is_not_this_results_fault() {
    let mut state = RuleState::default();
    state
      .flock
      .insert("k".into(), json!("x".repeat(MAX_RULE_FLOCK_STATE_BYTES)));
    assert!(check_with(&state, json!({ "state": { "pigeon": { "a": 1 } } })).is_ok());
    assert!(check_with(&state, json!({})).is_ok());
  }

  fn check_with(state: &RuleState, out: serde_json::Value) -> Result<RuleResult, RuleViolation> {
    validate_result(&out.to_string(), state)
  }

  #[test]
  fn deeply_nested_json_is_refused_not_overflowed() {
    let deep = "[".repeat(200) + &"]".repeat(200);
    let out = r#"{"state":{"flock":{"k":"#.to_string() + &deep + "}}}";
    let v = validate_result(&out, &empty_state()).unwrap_err();
    is(&v, "result", None, RuleProblem::NotJson);
  }

  #[test]
  fn reserved_names_take_precedence_over_unknown_ones() {
    let v = refused(json!({ "aaa": 1, "webhook": "https://example.com" }));
    is(&v, "webhook", None, RuleProblem::NotAvailableYet);
  }
}
