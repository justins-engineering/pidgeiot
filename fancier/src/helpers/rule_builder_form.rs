//! The rule builder form's plumbing: the rows a new rule starts from, what each row kind is
//! called in the form, and how the form's text fields read into and out of a
//! [`capsules::RuleModel`]. The checks and the sentence are capsules', so the form says what
//! dovecote will accept.

use capsules::{
  RULE_MODEL_VERSION, RuleAction, RuleCompareOp, RuleCondition, RuleMailRepeat, RuleMatch,
  RuleModel, RuleOperand, RuleSeverity, RuleTrigger, TelemetryHistoryPoint,
};

/// What a new rule opens on: mail when a sign runs hot for five minutes.
pub fn starter_model() -> RuleModel {
  RuleModel {
    v: RULE_MODEL_VERSION,
    when: RuleTrigger::Telemetry {
      key: "temp_c".to_string(),
    },
    r#match: RuleMatch::All,
    only_if: vec![RuleCondition::Compare {
      key: "temp_c".to_string(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(40.0),
      for_secs: 300,
    }],
    then: vec![RuleAction::Mail {
      severity: RuleSeverity::Warning,
      subject: "Running hot: {pigeon} at {temp_c} C".to_string(),
      body: String::new(),
      repeat: RuleMailRepeat::StartStop,
    }],
    otherwise: Vec::new(),
  }
}

/// The trigger's key.
pub fn trigger_key(model: &RuleModel) -> &str {
  let RuleTrigger::Telemetry { key } = &model.when;
  key
}

/// Every row kind the form offers, as (value, label), comparisons first.
pub const CONDITION_KINDS: [(&str, &str); 11] = [
  ("above", "is above"),
  ("below", "is below"),
  ("at_least", "is at least"),
  ("at_most", "is at most"),
  ("equals", "is"),
  ("not_equals", "is not"),
  ("between", "is between"),
  ("changed", "changes"),
  ("changed_by", "changes by more than"),
  ("bit_set", "has bit"),
  ("counted", "is reported more than"),
];

/// The comparisons a value table row offers, as (value, label).
pub const BAND_KINDS: [(&str, &str); 6] = [
  ("above", "is above"),
  ("below", "is below"),
  ("at_least", "is at least"),
  ("at_most", "is at most"),
  ("equals", "is"),
  ("not_equals", "is not"),
];

fn op_value(op: RuleCompareOp) -> &'static str {
  match op {
    RuleCompareOp::Above => "above",
    RuleCompareOp::Below => "below",
    RuleCompareOp::AtLeast => "at_least",
    RuleCompareOp::AtMost => "at_most",
    RuleCompareOp::Equals => "equals",
    RuleCompareOp::NotEquals => "not_equals",
  }
}

fn op_of(value: &str) -> Option<RuleCompareOp> {
  Some(match value {
    "above" => RuleCompareOp::Above,
    "below" => RuleCompareOp::Below,
    "at_least" => RuleCompareOp::AtLeast,
    "at_most" => RuleCompareOp::AtMost,
    "equals" => RuleCompareOp::Equals,
    "not_equals" => RuleCompareOp::NotEquals,
    _ => return None,
  })
}

/// The kind select's value for a row.
pub fn condition_kind(c: &RuleCondition) -> &'static str {
  match c {
    RuleCondition::Compare { op, .. } => op_value(*op),
    RuleCondition::Between { .. } => "between",
    RuleCondition::Changed { .. } => "changed",
    RuleCondition::ChangedBy { .. } => "changed_by",
    RuleCondition::BitSet { .. } => "bit_set",
    RuleCondition::Counted { .. } => "counted",
  }
}

/// The key a row names.
pub fn condition_key(c: &RuleCondition) -> &str {
  match c {
    RuleCondition::Compare { key, .. }
    | RuleCondition::Between { key, .. }
    | RuleCondition::Changed { key }
    | RuleCondition::ChangedBy { key, .. }
    | RuleCondition::BitSet { key, .. }
    | RuleCondition::Counted { key, .. } => key,
  }
}

/// Sets the key a row names.
pub fn set_condition_key(c: &mut RuleCondition, new: String) {
  match c {
    RuleCondition::Compare { key, .. }
    | RuleCondition::Between { key, .. }
    | RuleCondition::Changed { key }
    | RuleCondition::ChangedBy { key, .. }
    | RuleCondition::BitSet { key, .. }
    | RuleCondition::Counted { key, .. } => *key = new,
  }
}

/// The row a kind select change makes: the same key, and from one comparison to another the same
/// value and wait; any other change starts the row's own fields afresh.
pub fn with_kind(c: &RuleCondition, kind: &str) -> RuleCondition {
  let key = condition_key(c).to_string();
  if let Some(op) = op_of(kind) {
    if let RuleCondition::Compare { to, for_secs, .. } = c {
      return RuleCondition::Compare {
        key,
        op,
        to: to.clone(),
        for_secs: *for_secs,
      };
    }
    return RuleCondition::Compare {
      key,
      op,
      to: RuleOperand::Number(0.0),
      for_secs: 0,
    };
  }
  match kind {
    "between" => RuleCondition::Between {
      key,
      low: 0.0,
      high: 100.0,
      for_secs: 0,
    },
    "changed_by" => RuleCondition::ChangedBy {
      key,
      more_than: 1.0,
    },
    "bit_set" => RuleCondition::BitSet { key, bit: 0 },
    "counted" => RuleCondition::Counted {
      key,
      more_than: 3,
      within_secs: 86_400,
    },
    _ => RuleCondition::Changed { key },
  }
}

/// A new Only if row on `key`.
pub fn new_condition(key: &str) -> RuleCondition {
  RuleCondition::Compare {
    key: key.to_string(),
    op: RuleCompareOp::Above,
    to: RuleOperand::Number(0.0),
    for_secs: 0,
  }
}

/// A new mail action.
pub fn new_mail() -> RuleAction {
  RuleAction::Mail {
    severity: RuleSeverity::Warning,
    subject: "Check {pigeon}".to_string(),
    body: String::new(),
    repeat: RuleMailRepeat::StartStop,
  }
}

/// A new value action.
pub fn new_value() -> RuleAction {
  RuleAction::SetValue {
    key: String::new(),
    value: String::new(),
    bands: Vec::new(),
  }
}

/// A number field's text: empty for a value that is not a number yet.
pub fn number_text(n: f64) -> String {
  if n.is_finite() {
    n.to_string()
  } else {
    String::new()
  }
}

/// A number field's value; not a number (refused by the checks as "enter a number") when the
/// text is not one.
pub fn parse_number(text: &str) -> f64 {
  text.trim().parse::<f64>().unwrap_or(f64::NAN)
}

/// A whole count from a field, clamped into `0..=max` so the checks name the cap instead of the
/// field overflowing.
pub fn parse_count(text: &str, max: u32) -> u32 {
  let n = parse_number(text);
  if n.is_finite() && n > 0.0 {
    (n.round() as u64).min(u64::from(max)) as u32
  } else {
    0
  }
}

/// Seconds from a field in `unit` seconds, such as minutes; 0 when blank or not a number.
pub fn secs_from(text: &str, unit: u32) -> u32 {
  let n = parse_number(text);
  if n.is_finite() && n > 0.0 {
    (n * f64::from(unit)).round().min(f64::from(u32::MAX)) as u32
  } else {
    0
  }
}

/// A field's text for `secs` in `unit` seconds: blank for 0, else up to two decimals.
pub fn secs_text(secs: u32, unit: u32) -> String {
  if secs == 0 {
    return String::new();
  }
  let n = (f64::from(secs) / f64::from(unit) * 100.0).round() / 100.0;
  n.to_string()
}

/// What the right side of a comparison is: a typed value, or another key.
pub fn operand_is_key(to: &RuleOperand) -> bool {
  matches!(to, RuleOperand::Key(_))
}

/// The right side's text.
pub fn operand_text(to: &RuleOperand) -> String {
  match to {
    RuleOperand::Number(n) => number_text(*n),
    RuleOperand::Text(t) | RuleOperand::Key(t) => t.clone(),
  }
}

/// The right side from its field: another key's name, or a typed value, a number when it reads
/// as one and a word otherwise.
pub fn operand_from(text: &str, is_key: bool) -> RuleOperand {
  if is_key {
    return RuleOperand::Key(text.trim().to_string());
  }
  let n = parse_number(text);
  if n.is_finite() {
    RuleOperand::Number(n)
  } else {
    RuleOperand::Text(text.to_string())
  }
}

/// Every key the flock reported in `points`, sorted: text keys too, since a rule can compare or
/// watch a word.
pub fn keys_from_history(points: &[TelemetryHistoryPoint]) -> Vec<String> {
  let mut keys: Vec<String> = points.iter().map(|p| p.key.clone()).collect();
  keys.sort();
  keys.dedup();
  keys
}

/// The severity select's value and label, in the form's words.
pub const SEVERITIES: [(&str, &str); 3] = [
  ("info", "For the record"),
  ("warning", "Worth a look"),
  ("critical", "Needs action now"),
];

/// The select value of a severity.
pub fn severity_value(s: RuleSeverity) -> &'static str {
  match s {
    RuleSeverity::Info => "info",
    RuleSeverity::Warning => "warning",
    RuleSeverity::Critical => "critical",
  }
}

/// The severity a select value names.
pub fn severity_of(value: &str) -> RuleSeverity {
  match value {
    "info" => RuleSeverity::Info,
    "critical" => RuleSeverity::Critical,
    _ => RuleSeverity::Warning,
  }
}

/// The comparison a band select value names.
pub fn band_op(value: &str) -> RuleCompareOp {
  op_of(value).unwrap_or(RuleCompareOp::Above)
}

/// A band select's value.
pub fn band_op_value(op: RuleCompareOp) -> &'static str {
  op_value(op)
}

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::{rule_model_test_cases, validate_rule_model};

  #[test]
  fn the_starter_is_a_valid_rule_with_its_own_test_cases() {
    let model = starter_model();
    assert_eq!(validate_rule_model(&model), Ok(()));
    assert_eq!(rule_model_test_cases(&model).len(), 2);
    assert_eq!(trigger_key(&model), "temp_c");
  }

  #[test]
  fn changing_a_rows_kind_keeps_what_still_applies() {
    let held = RuleCondition::Compare {
      key: "temp_c".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(40.0),
      for_secs: 300,
    };
    assert_eq!(
      with_kind(&held, "below"),
      RuleCondition::Compare {
        key: "temp_c".into(),
        op: RuleCompareOp::Below,
        to: RuleOperand::Number(40.0),
        for_secs: 300,
      }
    );
    let between = with_kind(&held, "between");
    assert_eq!(condition_kind(&between), "between");
    assert_eq!(condition_key(&between), "temp_c");
    let back = with_kind(&between, "equals");
    assert_eq!(condition_kind(&back), "equals");
    for (kind, _) in CONDITION_KINDS {
      let row = with_kind(&held, kind);
      assert_eq!(condition_kind(&row), kind);
      let mut model = starter_model();
      model.only_if = vec![row];
      assert_eq!(validate_rule_model(&model), Ok(()), "{kind} starts valid");
    }
  }

  #[test]
  fn number_fields_read_what_a_person_types() {
    assert_eq!(parse_number(" 40.5 "), 40.5);
    assert!(parse_number("forty").is_nan());
    assert!(parse_number("").is_nan());
    assert_eq!(number_text(f64::NAN), "");
    assert_eq!(number_text(40.0), "40");
    assert_eq!(parse_count("3.6", 31), 4);
    assert_eq!(parse_count("99", 31), 31);
    assert_eq!(parse_count("-2", 31), 0);
    assert_eq!(secs_from("5", 60), 300);
    assert_eq!(secs_from("1.5", 3_600), 5_400);
    assert_eq!(secs_from("", 60), 0);
    assert_eq!(secs_text(300, 60), "5");
    assert_eq!(secs_text(0, 60), "");
    assert_eq!(secs_text(90, 60), "1.5");
  }

  #[test]
  fn a_typed_value_is_a_number_when_it_reads_as_one() {
    assert_eq!(operand_from("40", false), RuleOperand::Number(40.0));
    assert_eq!(
      operand_from("night", false),
      RuleOperand::Text("night".into())
    );
    assert_eq!(
      operand_from(" temp_out ", true),
      RuleOperand::Key("temp_out".into())
    );
    assert_eq!(operand_text(&RuleOperand::Number(2.5)), "2.5");
    assert!(operand_is_key(&RuleOperand::Key("k".into())));
  }

  #[test]
  fn the_key_picker_offers_text_keys_too() {
    let point = |key: &str, num: Option<f64>| TelemetryHistoryPoint {
      pigeon_id: "p".into(),
      key: key.into(),
      value: "v".into(),
      value_num: num,
      reported_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let keys = keys_from_history(&[
      point("temp_c", Some(1.0)),
      point("layout", None),
      point("temp_c", Some(2.0)),
    ]);
    assert_eq!(keys, ["layout", "temp_c"]);
  }

  #[test]
  fn selects_round_trip_their_values() {
    for (value, _) in SEVERITIES {
      assert_eq!(severity_value(severity_of(value)), value);
    }
    for (value, _) in BAND_KINDS {
      assert_eq!(band_op_value(band_op(value)), value);
    }
  }
}
