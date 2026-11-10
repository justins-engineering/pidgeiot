//! The rule builder's model: the When, Only if, Then and Otherwise rows a person fills in on the
//! rule page, the checks fancier and dovecote both apply to it, the words the page shows for it,
//! the test cases a dry run replays for it, and the three-line module that runs it.
//!
//! A builder rule's module holds no logic of its own: it hands its model, parsed from a JSON
//! string, to `dovecote/rules/builder.js`, the one interpreter every builder rule shares. So the
//! semantics written here (alert keys, what a hold or a count means, which reading decides) are
//! that file's, and `dovecote/rules/run-builder.mjs` holds the two together.

use crate::{
  MAX_RULE_ALERT_BODY_BYTES, MAX_RULE_ALERT_SUBJECT_BYTES, MAX_TELEMETRY_KEY_BYTES,
  MAX_TELEMETRY_VALUE_BYTES, RuleAlertKind, RuleCaseEvent, RuleCaseMail, RuleCaseMetric,
  RuleCasePigeon, RuleCaseReading, RuleSeverity, RuleTestCase,
};
use serde::{Deserialize, Serialize};

/// The model shape this build writes and `builder.js` reads.
pub const RULE_MODEL_VERSION: u32 = 1;

/// The interpreter's module, which a builder rule imports beside the helpers.
pub const RULE_BUILDER_IMPORT: &str = "./builder.js";

/// Most Only if rows.
pub const MAX_MODEL_CONDITIONS: usize = 16;
/// Most actions in Then, and again in Otherwise.
pub const MAX_MODEL_ACTIONS: usize = 8;
/// Most mail actions in a model: one alert each per batch, inside the result's eight.
pub const MAX_MODEL_MAILS: usize = 4;
/// Most "reported more than" rows, whose reading times the pigeon's state document keeps.
pub const MAX_MODEL_COUNTED: usize = 4;
/// Most rows of one value's table.
pub const MAX_MODEL_BANDS: usize = 16;
/// The highest count a "reported more than" row may name: the state keeps one time more.
pub const MAX_MODEL_COUNT: u32 = 31;
/// The longest hold or count window, a week.
pub const MAX_MODEL_WINDOW_SECS: u32 = 7 * 86_400;
/// Most bytes of a model's compact JSON, which keeps its module far under the source cap.
pub const MAX_RULE_MODEL_BYTES: usize = 16 * 1024;

/// A rule as the builder holds it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleModel {
  /// [`RULE_MODEL_VERSION`].
  pub v: u32,
  /// What starts a run.
  pub when: RuleTrigger,
  /// How the Only if rows combine.
  #[serde(default)]
  pub r#match: RuleMatch,
  /// The conditions; none means every report of the key acts.
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub only_if: Vec<RuleCondition>,
  /// What happens while the conditions hold.
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub then: Vec<RuleAction>,
  /// What happens while they do not.
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub otherwise: Vec<RuleAction>,
}

/// What starts a run. Only telemetry exists; schedules and other triggers extend this enum.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleTrigger {
  /// Each reading of one pigeon that carries `key`.
  Telemetry {
    /// The telemetry key.
    key: String,
  },
}

/// How the Only if rows combine.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleMatch {
  /// Every row holds.
  #[default]
  All,
  /// At least one row holds.
  Any,
}

/// One Only if row. Each names a key, read as of the reading being decided: its value in that
/// reading or the latest earlier reading of the same batch, and no row holds for a key the batch
/// has not reported yet.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "is", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleCondition {
  /// `key` against a number, a text or another key.
  Compare {
    /// The telemetry key compared.
    key: String,
    /// How it is compared.
    op: RuleCompareOp,
    /// What it is compared with.
    to: RuleOperand,
    /// Holds only once true this long without a break; 0 for at once.
    #[serde(default, skip_serializing_if = "is_zero")]
    for_secs: u32,
  },
  /// `key` from `low` to `high`, both included.
  Between {
    /// The telemetry key, read as a number.
    key: String,
    /// The low end.
    low: f64,
    /// The high end, at or above `low`.
    high: f64,
    /// As for [`RuleCondition::Compare`].
    #[serde(default, skip_serializing_if = "is_zero")]
    for_secs: u32,
  },
  /// The reading carries `key` with a value other than the one it displaced.
  Changed {
    /// The telemetry key.
    key: String,
  },
  /// The reading moved `key` by more than `more_than` from the value it displaced.
  ChangedBy {
    /// The telemetry key, read as a number.
    key: String,
    /// The least change that does not count, never negative.
    more_than: f64,
  },
  /// `key` is an integer with bit `bit` (0 to 30) set.
  BitSet {
    /// The telemetry key.
    key: String,
    /// The bit, 0 for the lowest.
    bit: u8,
  },
  /// Readings carrying `key` within the last `within_secs` number more than `more_than`. Times
  /// are counted once each, so readings stamped the same second, or clamped to the 24-hour
  /// backdate boundary together, are one report.
  Counted {
    /// The telemetry key whose reports are counted.
    key: String,
    /// The count to exceed, up to [`MAX_MODEL_COUNT`].
    more_than: u32,
    /// The window, a minute to [`MAX_MODEL_WINDOW_SECS`].
    within_secs: u32,
  },
}

/// A comparison. On text, only [`RuleCompareOp::Equals`] and [`RuleCompareOp::NotEquals`].
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleCompareOp {
  /// Greater than.
  Above,
  /// Less than.
  Below,
  /// Greater than or equal.
  AtLeast,
  /// Less than or equal.
  AtMost,
  /// Equal.
  Equals,
  /// Not equal.
  NotEquals,
}

/// The right side of a comparison.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleOperand {
  /// A fixed number.
  Number(f64),
  /// A fixed text, compared exactly.
  Text(String),
  /// Another key's value as of the same reading: as numbers when both parse, else as text.
  Key(String),
}

/// One Then or Otherwise action.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleAction {
  /// A mail to the rule's recipients. `{pigeon}`, `{flock}` and `{<key>}` in the texts take the
  /// pigeon's name, the flock's name and the key's latest value in the batch.
  Mail {
    /// The mail's severity.
    severity: RuleSeverity,
    /// The subject, never blank.
    subject: String,
    /// The body; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    body: String,
    /// How often it mails.
    #[serde(default)]
    repeat: RuleMailRepeat,
  },
  /// A derived value: the first band whose condition holds gives it, else `value`. Written only
  /// when it changes. Placeholders as for mail.
  SetValue {
    /// The derived value's key.
    key: String,
    /// Its value when no band holds.
    value: String,
    /// The table, checked in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bands: Vec<RuleBand>,
  },
}

/// How often a mail action mails.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleMailRepeat {
  /// Once when its branch starts applying and once when it stops.
  #[default]
  StartStop,
  /// Once each time its branch starts applying, even inside one batch.
  Once,
}

/// One row of a value's table. Its condition cannot hold or count, since a table row keeps no
/// state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleBand {
  /// The row's condition, `if` on the wire.
  #[serde(rename = "if")]
  pub when: RuleCondition,
  /// The value it gives when its condition holds.
  pub value: String,
}

fn is_zero(n: &u32) -> bool {
  *n == 0
}

/// Which branch an action sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleBranch {
  /// Then.
  Then,
  /// Otherwise.
  Otherwise,
}

impl RuleBranch {
  /// The branch's own word, as its row heading says it.
  pub fn label(self) -> &'static str {
    match self {
      RuleBranch::Then => "Then",
      RuleBranch::Otherwise => "Otherwise",
    }
  }

  fn key_prefix(self) -> &'static str {
    match self {
      RuleBranch::Then => "then-",
      RuleBranch::Otherwise => "else-",
    }
  }
}

/// Where in the model a problem sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleModelPlace {
  /// The model as a whole.
  Rule,
  /// The When row.
  When,
  /// An Only if row, from 0.
  Condition(usize),
  /// An action, from 0.
  Action(RuleBranch, usize),
  /// A row of an action's value table, both from 0.
  Band(RuleBranch, usize, usize),
}

/// What is wrong at a [`RuleModelPlace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleModelProblem {
  /// A model version this build does not know.
  UnknownVersion,
  /// The model's JSON is over [`MAX_RULE_MODEL_BYTES`].
  TooLarge,
  /// Neither Then nor Otherwise does anything.
  NoAction,
  /// More rows than the cap.
  TooManyConditions,
  /// More actions in one branch than the cap.
  TooManyActions,
  /// More mail actions than the cap.
  TooManyMails,
  /// More "reported more than" rows than the cap.
  TooManyCounted,
  /// More value table rows than the cap.
  TooManyBands,
  /// A key is blank.
  EmptyKey,
  /// A key is over the telemetry key cap.
  KeyTooLong,
  /// A key holds a control character.
  KeyControl,
  /// A key named `__proto__`, which JavaScript reads as no key at all.
  ReservedKey,
  /// A number that is not finite.
  NotFinite,
  /// A text compared with something other than "is" or "is not".
  TextOrder,
  /// A range whose low end is above its high end.
  LowAboveHigh,
  /// A negative amount of change.
  Negative,
  /// A bit outside 0 to 30.
  BitRange,
  /// A count above [`MAX_MODEL_COUNT`].
  CountTooHigh,
  /// A hold or window over [`MAX_MODEL_WINDOW_SECS`], or a window under a minute.
  WindowRange,
  /// A hold or a count inside a value table.
  StatefulBand,
  /// A blank subject.
  EmptySubject,
  /// A subject over the mail cap.
  SubjectTooLong,
  /// A control character in a subject, or one other than newline in a body.
  TextControl,
  /// A body over the mail cap.
  BodyTooLong,
  /// A value or a compared text over the telemetry value cap.
  TextTooLong,
}

/// The first problem in a model, as the form shows it and the route answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleModelError {
  /// The section and row it is in.
  pub place: RuleModelPlace,
  /// What is wrong there.
  pub problem: RuleModelProblem,
}

impl RuleModelProblem {
  /// The problem in the form's words.
  pub fn message(self) -> &'static str {
    match self {
      RuleModelProblem::UnknownVersion => "this rule was made by a newer builder",
      RuleModelProblem::TooLarge => "the rule is too large",
      RuleModelProblem::NoAction => "add at least one thing to do under Then or Otherwise",
      RuleModelProblem::TooManyConditions => "at most 16 conditions",
      RuleModelProblem::TooManyActions => "at most 8 actions here",
      RuleModelProblem::TooManyMails => "at most 4 mails in one rule",
      RuleModelProblem::TooManyCounted => "at most 4 \u{201c}reported more than\u{201d} conditions",
      RuleModelProblem::TooManyBands => "at most 16 rows in a value table",
      RuleModelProblem::EmptyKey => "pick a telemetry key",
      RuleModelProblem::KeyTooLong => "a key is at most 128 bytes",
      RuleModelProblem::KeyControl => "a key cannot hold a control character",
      RuleModelProblem::ReservedKey => "a key cannot be named __proto__",
      RuleModelProblem::NotFinite => "enter a number",
      RuleModelProblem::TextOrder => {
        "text can only be compared with \u{201c}is\u{201d} or \u{201c}is not\u{201d}"
      }
      RuleModelProblem::LowAboveHigh => "the first number must not be above the second",
      RuleModelProblem::Negative => "the amount cannot be negative",
      RuleModelProblem::BitRange => "a bit is a number from 0 to 30",
      RuleModelProblem::CountTooHigh => "count up to 31 times",
      RuleModelProblem::WindowRange => "the time must be from 1 minute to 7 days",
      RuleModelProblem::StatefulBand => "a table row cannot wait or count",
      RuleModelProblem::EmptySubject => "the mail needs a subject",
      RuleModelProblem::SubjectTooLong => "a subject is at most 120 bytes",
      RuleModelProblem::TextControl => "remove the control character",
      RuleModelProblem::BodyTooLong => "a mail's text is at most 1,024 bytes",
      RuleModelProblem::TextTooLong => "a value is at most 1,024 bytes",
    }
  }
}

impl std::fmt::Display for RuleModelError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let row = |f: &mut std::fmt::Formatter<'_>, label: &str, i: usize| {
      f.write_str(label)?;
      f.write_str(", row ")?;
      f.write_str(&(i + 1).to_string())
    };
    match self.place {
      RuleModelPlace::Rule => f.write_str("The rule")?,
      RuleModelPlace::When => f.write_str("When")?,
      RuleModelPlace::Condition(i) => row(f, "Only if", i)?,
      RuleModelPlace::Action(branch, i) => row(f, branch.label(), i)?,
      RuleModelPlace::Band(branch, i, j) => {
        row(f, branch.label(), i)?;
        f.write_str(", table row ")?;
        f.write_str(&(j + 1).to_string())?;
      }
    }
    f.write_str(": ")?;
    f.write_str(self.problem.message())
  }
}

fn fail(place: RuleModelPlace, problem: RuleModelProblem) -> Result<(), RuleModelError> {
  Err(RuleModelError { place, problem })
}

fn check_key(key: &str, place: RuleModelPlace) -> Result<(), RuleModelError> {
  if key.trim().is_empty() {
    return fail(place, RuleModelProblem::EmptyKey);
  }
  if key.len() > MAX_TELEMETRY_KEY_BYTES {
    return fail(place, RuleModelProblem::KeyTooLong);
  }
  if key.chars().any(char::is_control) {
    return fail(place, RuleModelProblem::KeyControl);
  }
  if key == "__proto__" {
    return fail(place, RuleModelProblem::ReservedKey);
  }
  Ok(())
}

fn check_number(n: f64, place: RuleModelPlace) -> Result<(), RuleModelError> {
  if n.is_finite() {
    Ok(())
  } else {
    fail(place, RuleModelProblem::NotFinite)
  }
}

fn check_text(text: &str, place: RuleModelPlace) -> Result<(), RuleModelError> {
  if text.len() > MAX_TELEMETRY_VALUE_BYTES {
    return fail(place, RuleModelProblem::TextTooLong);
  }
  if text.contains('\0') {
    return fail(place, RuleModelProblem::TextControl);
  }
  Ok(())
}

fn check_hold(secs: u32, place: RuleModelPlace) -> Result<(), RuleModelError> {
  if secs > MAX_MODEL_WINDOW_SECS {
    return fail(place, RuleModelProblem::WindowRange);
  }
  Ok(())
}

fn check_condition(c: &RuleCondition, place: RuleModelPlace) -> Result<(), RuleModelError> {
  match c {
    RuleCondition::Compare {
      key,
      op,
      to,
      for_secs,
    } => {
      check_key(key, place)?;
      match to {
        RuleOperand::Number(n) => check_number(*n, place)?,
        RuleOperand::Text(t) => {
          check_text(t, place)?;
          if !matches!(op, RuleCompareOp::Equals | RuleCompareOp::NotEquals) {
            return fail(place, RuleModelProblem::TextOrder);
          }
        }
        RuleOperand::Key(other) => check_key(other, place)?,
      }
      check_hold(*for_secs, place)
    }
    RuleCondition::Between {
      key,
      low,
      high,
      for_secs,
    } => {
      check_key(key, place)?;
      check_number(*low, place)?;
      check_number(*high, place)?;
      if low > high {
        return fail(place, RuleModelProblem::LowAboveHigh);
      }
      check_hold(*for_secs, place)
    }
    RuleCondition::Changed { key } => check_key(key, place),
    RuleCondition::ChangedBy { key, more_than } => {
      check_key(key, place)?;
      check_number(*more_than, place)?;
      if *more_than < 0.0 {
        return fail(place, RuleModelProblem::Negative);
      }
      Ok(())
    }
    RuleCondition::BitSet { key, bit } => {
      check_key(key, place)?;
      if *bit > 30 {
        return fail(place, RuleModelProblem::BitRange);
      }
      Ok(())
    }
    RuleCondition::Counted {
      key,
      more_than,
      within_secs,
    } => {
      check_key(key, place)?;
      if *more_than > MAX_MODEL_COUNT {
        return fail(place, RuleModelProblem::CountTooHigh);
      }
      if !(60..=MAX_MODEL_WINDOW_SECS).contains(within_secs) {
        return fail(place, RuleModelProblem::WindowRange);
      }
      Ok(())
    }
  }
}

fn stateful(c: &RuleCondition) -> bool {
  match c {
    RuleCondition::Compare { for_secs, .. } | RuleCondition::Between { for_secs, .. } => {
      *for_secs > 0
    }
    RuleCondition::Counted { .. } => true,
    _ => false,
  }
}

fn check_action(a: &RuleAction, branch: RuleBranch, i: usize) -> Result<(), RuleModelError> {
  let place = RuleModelPlace::Action(branch, i);
  match a {
    RuleAction::Mail { subject, body, .. } => {
      if subject.trim().is_empty() {
        return fail(place, RuleModelProblem::EmptySubject);
      }
      if subject.len() > MAX_RULE_ALERT_SUBJECT_BYTES {
        return fail(place, RuleModelProblem::SubjectTooLong);
      }
      if subject.chars().any(char::is_control) {
        return fail(place, RuleModelProblem::TextControl);
      }
      if body.len() > MAX_RULE_ALERT_BODY_BYTES {
        return fail(place, RuleModelProblem::BodyTooLong);
      }
      if body.chars().any(|c| c.is_control() && c != '\n') {
        return fail(place, RuleModelProblem::TextControl);
      }
      Ok(())
    }
    RuleAction::SetValue { key, value, bands } => {
      check_key(key, place)?;
      check_text(value, place)?;
      if bands.len() > MAX_MODEL_BANDS {
        return fail(place, RuleModelProblem::TooManyBands);
      }
      for (j, band) in bands.iter().enumerate() {
        let at = RuleModelPlace::Band(branch, i, j);
        check_condition(&band.when, at)?;
        if stateful(&band.when) {
          return fail(at, RuleModelProblem::StatefulBand);
        }
        check_text(&band.value, at)?;
      }
      Ok(())
    }
  }
}

/// Checks a model before it is saved or compiled: its version, every cap, every key, number
/// and text. The first problem found, in row order.
pub fn validate_rule_model(model: &RuleModel) -> Result<(), RuleModelError> {
  if model.v != RULE_MODEL_VERSION {
    return fail(RuleModelPlace::Rule, RuleModelProblem::UnknownVersion);
  }
  let RuleTrigger::Telemetry { key } = &model.when;
  check_key(key, RuleModelPlace::When)?;
  if model.only_if.len() > MAX_MODEL_CONDITIONS {
    return fail(RuleModelPlace::Rule, RuleModelProblem::TooManyConditions);
  }
  let mut counted = 0;
  for (i, c) in model.only_if.iter().enumerate() {
    check_condition(c, RuleModelPlace::Condition(i))?;
    if matches!(c, RuleCondition::Counted { .. }) {
      counted += 1;
      if counted > MAX_MODEL_COUNTED {
        return fail(
          RuleModelPlace::Condition(i),
          RuleModelProblem::TooManyCounted,
        );
      }
    }
  }
  if model.then.is_empty() && model.otherwise.is_empty() {
    return fail(RuleModelPlace::Rule, RuleModelProblem::NoAction);
  }
  let mut mails = 0;
  for (branch, actions) in [
    (RuleBranch::Then, &model.then),
    (RuleBranch::Otherwise, &model.otherwise),
  ] {
    if actions.len() > MAX_MODEL_ACTIONS {
      return fail(
        RuleModelPlace::Action(branch, MAX_MODEL_ACTIONS),
        RuleModelProblem::TooManyActions,
      );
    }
    for (i, a) in actions.iter().enumerate() {
      check_action(a, branch, i)?;
      if matches!(a, RuleAction::Mail { .. }) {
        mails += 1;
        if mails > MAX_MODEL_MAILS {
          return fail(
            RuleModelPlace::Action(branch, i),
            RuleModelProblem::TooManyMails,
          );
        }
      }
    }
  }
  if model_json(model).len() > MAX_RULE_MODEL_BYTES {
    return fail(RuleModelPlace::Rule, RuleModelProblem::TooLarge);
  }
  Ok(())
}

fn model_json(model: &RuleModel) -> String {
  serde_json::to_string(model).expect("a rule model always serializes")
}

const MODULE_HEAD: &str = "// Made by the rule builder: edit it there, or convert it to code.\n\
  import { run } from \"./builder.js\";\n\nconst MODEL = JSON.parse(";
const MODULE_TAIL: &str = ");\n\nexport default (event) => run(MODEL, event);\n";

/// The module a builder version stores and deploys: the model as a JSON string literal handed to
/// `JSON.parse`, never as an object literal, where a `__proto__` key would set a prototype
/// rather than name a key. Call it on a validated model.
pub fn compile_rule_model(model: &RuleModel) -> String {
  let literal = serde_json::to_string(&model_json(model)).expect("a JSON string always serializes");
  let mut source = String::with_capacity(MODULE_HEAD.len() + literal.len() + MODULE_TAIL.len());
  source.push_str(MODULE_HEAD);
  source.push_str(&literal);
  source.push_str(MODULE_TAIL);
  source
}

/// The alert key a mail action sends under. A start-and-stop mail keeps one key; a once mail
/// adds the number of the start it mails for, counted per pigeon, so each start mails once.
pub fn mail_alert_key(branch: RuleBranch, index: usize) -> String {
  let i = index.to_string();
  let mut key = String::with_capacity(5 + i.len());
  key.push_str(branch.key_prefix());
  key.push_str(&i);
  key
}

/// The mail action an alert key names, when it is one of this model's.
pub fn mail_action_for_key<'a>(
  model: &'a RuleModel,
  key: &str,
) -> Option<(RuleBranch, &'a RuleAction)> {
  let (branch, rest) = if let Some(rest) = key.strip_prefix("then-") {
    (RuleBranch::Then, rest)
  } else {
    (RuleBranch::Otherwise, key.strip_prefix("else-")?)
  };
  let index: usize = rest.split('-').next()?.parse().ok()?;
  let actions = match branch {
    RuleBranch::Then => &model.then,
    RuleBranch::Otherwise => &model.otherwise,
  };
  actions
    .get(index)
    .filter(|a| matches!(a, RuleAction::Mail { .. }))
    .map(|a| (branch, a))
}

fn number_words(n: f64) -> String {
  n.to_string()
}

fn quoted(text: &str) -> String {
  ["\u{201c}", text, "\u{201d}"].concat()
}

/// A duration as a person says it: "5 minutes", "2 hours", "90 seconds".
pub fn duration_words(secs: u32) -> String {
  let (n, one, many) = if secs >= 86_400 && secs.is_multiple_of(86_400) {
    (secs / 86_400, " day", " days")
  } else if secs >= 3_600 && secs.is_multiple_of(3_600) {
    (secs / 3_600, " hour", " hours")
  } else if secs >= 60 && secs.is_multiple_of(60) {
    (secs / 60, " minute", " minutes")
  } else {
    (secs, " second", " seconds")
  };
  [n.to_string().as_str(), if n == 1 { one } else { many }].concat()
}

/// A comparison in the form's words.
pub fn compare_words(op: RuleCompareOp) -> &'static str {
  match op {
    RuleCompareOp::Above => "is above",
    RuleCompareOp::Below => "is below",
    RuleCompareOp::AtLeast => "is at least",
    RuleCompareOp::AtMost => "is at most",
    RuleCompareOp::Equals => "is",
    RuleCompareOp::NotEquals => "is not",
  }
}

/// One Only if row as a phrase, such as "temp_c is above 40 for 5 minutes".
pub fn condition_words(c: &RuleCondition) -> String {
  let hold = |text: String, secs: u32| {
    if secs == 0 {
      return text;
    }
    [text.as_str(), " for ", &duration_words(secs)].concat()
  };
  match c {
    RuleCondition::Compare {
      key,
      op,
      to,
      for_secs,
    } => {
      let right = match to {
        RuleOperand::Number(n) => number_words(*n),
        RuleOperand::Text(t) => quoted(t),
        RuleOperand::Key(k) => k.clone(),
      };
      hold(
        [key.as_str(), " ", compare_words(*op), " ", &right].concat(),
        *for_secs,
      )
    }
    RuleCondition::Between {
      key,
      low,
      high,
      for_secs,
    } => hold(
      [
        key.as_str(),
        " is between ",
        &number_words(*low),
        " and ",
        &number_words(*high),
      ]
      .concat(),
      *for_secs,
    ),
    RuleCondition::Changed { key } => [key.as_str(), " changes"].concat(),
    RuleCondition::ChangedBy { key, more_than } => [
      key.as_str(),
      " changes by more than ",
      &number_words(*more_than),
    ]
    .concat(),
    RuleCondition::BitSet { key, bit } => {
      [key.as_str(), " has bit ", &bit.to_string(), " set"].concat()
    }
    RuleCondition::Counted {
      key,
      more_than,
      within_secs,
    } => {
      let times = if *more_than == 1 { " time" } else { " times" };
      [
        key.as_str(),
        " is reported more than ",
        &more_than.to_string(),
        times,
        " within ",
        &duration_words(*within_secs),
      ]
      .concat()
    }
  }
}

/// A severity in the form's words.
pub fn severity_words(severity: RuleSeverity) -> &'static str {
  match severity {
    RuleSeverity::Info => "for the record",
    RuleSeverity::Warning => "worth a look",
    RuleSeverity::Critical => "needs action now",
  }
}

/// One action as a phrase, such as "mail \u{201c}Running hot\u{201d} when it starts and stops".
pub fn action_words(a: &RuleAction) -> String {
  match a {
    RuleAction::Mail {
      severity,
      subject,
      repeat,
      ..
    } => {
      let when = match repeat {
        RuleMailRepeat::StartStop => ", when this starts and when it stops",
        RuleMailRepeat::Once => ", once each time this starts",
      };
      [
        "mail ",
        &quoted(subject),
        " (",
        severity_words(*severity),
        ")",
        when,
      ]
      .concat()
    }
    RuleAction::SetValue { key, value, bands } => {
      let mut text = ["set ", key.as_str(), " to "].concat();
      for band in bands {
        text.push_str(&quoted(&band.value));
        text.push_str(" if ");
        text.push_str(&condition_words(&band.when));
        text.push_str(", else ");
      }
      text.push_str(&quoted(value));
      text
    }
  }
}

fn actions_words(actions: &[RuleAction]) -> String {
  let parts: Vec<String> = actions.iter().map(action_words).collect();
  parts.join("; and ")
}

/// The whole rule as one plain paragraph, generated from the model so it always says what runs.
pub fn rule_model_sentence(model: &RuleModel) -> String {
  let RuleTrigger::Telemetry { key } = &model.when;
  let mut text = ["When a pigeon reports ", key.as_str()].concat();
  if !model.only_if.is_empty() {
    let join = match model.r#match {
      RuleMatch::All => " and ",
      RuleMatch::Any => " or ",
    };
    let parts: Vec<String> = model.only_if.iter().map(condition_words).collect();
    text.push_str(", if ");
    text.push_str(&parts.join(join));
  }
  text.push_str(": ");
  if model.then.is_empty() {
    text.push_str("nothing");
  } else {
    text.push_str(&actions_words(&model.then));
  }
  text.push('.');
  if !model.otherwise.is_empty() {
    text.push_str(" Otherwise: ");
    text.push_str(&actions_words(&model.otherwise));
    text.push('.');
  }
  text
}

/// A step away from `n` that `n` cannot absorb: 1, or a billionth of `n` where 1 is below its
/// precision.
fn case_step(n: f64) -> f64 {
  (n.abs() * 1e-9).max(1.0)
}

/// The value one row needs to hold (`true`) or not, for the rows test cases can be made from;
/// none when that value would not be a finite number.
fn case_value(c: &RuleCondition, holds: bool) -> Option<(&str, f64)> {
  let (key, value) = match c {
    RuleCondition::Compare {
      key,
      op,
      to: RuleOperand::Number(n),
      ..
    } => {
      let n = *n;
      let value = match (op, holds) {
        (RuleCompareOp::Above, true) | (RuleCompareOp::AtMost, false) => n + case_step(n),
        (RuleCompareOp::Below, true) | (RuleCompareOp::AtLeast, false) => n - case_step(n),
        (RuleCompareOp::Above | RuleCompareOp::Below, false) => n,
        (RuleCompareOp::AtLeast | RuleCompareOp::AtMost | RuleCompareOp::Equals, true) => n,
        (RuleCompareOp::NotEquals, false) => n,
        (RuleCompareOp::Equals, false) | (RuleCompareOp::NotEquals, true) => n + case_step(n),
      };
      (key, value)
    }
    RuleCondition::Between { key, low, high, .. } => (
      key,
      if holds {
        low / 2.0 + high / 2.0
      } else {
        high + case_step(*high)
      },
    ),
    _ => return None,
  };
  value.is_finite().then_some((key.as_str(), value))
}

fn hold_secs(c: &RuleCondition) -> u32 {
  match c {
    RuleCondition::Compare { for_secs, .. } | RuleCondition::Between { for_secs, .. } => *for_secs,
    _ => 0,
  }
}

/// The mails `actions` send over a run of batches whose decisions were `truths`, from empty
/// state: what `builder.js` returns, through the platform's fire, clear and notify rule.
fn expected_mails(
  actions: &[RuleAction],
  branch: RuleBranch,
  truths: &[bool],
) -> Vec<RuleCaseMail> {
  let active: Vec<bool> = truths
    .iter()
    .map(|t| *t == (branch == RuleBranch::Then))
    .collect();
  let mut mails = Vec::new();
  for (i, action) in actions.iter().enumerate() {
    let RuleAction::Mail { repeat, .. } = action else {
      continue;
    };
    let key = mail_alert_key(branch, i);
    let mut flag = false;
    let mut fired = false;
    // The interpreter starts from "not holding", so Otherwise has not started yet either.
    let mut prev = branch == RuleBranch::Otherwise;
    let mut starts = 0;
    for on in &active {
      match repeat {
        RuleMailRepeat::StartStop => {
          if *on != flag {
            if *on {
              mails.push(RuleCaseMail {
                key: key.clone(),
                kind: RuleAlertKind::Fire,
              });
              fired = true;
            } else if fired {
              mails.push(RuleCaseMail {
                key: key.clone(),
                kind: RuleAlertKind::Clear,
              });
            }
            flag = *on;
          }
        }
        RuleMailRepeat::Once => {
          if *on && !prev {
            starts += 1;
            let n = starts.to_string();
            mails.push(RuleCaseMail {
              key: [key.as_str(), "-", &n].concat(),
              kind: RuleAlertKind::Notify,
            });
          }
        }
      }
      prev = *on;
    }
  }
  mails
}

/// Two test cases made from the model, one where its conditions hold and one where they do not,
/// or none when a row is not a number comparison, two rows share a key, or there is no row. A
/// hold adds a second batch that much later, so each case shows what the hold waits for.
pub fn rule_model_test_cases(model: &RuleModel) -> Vec<RuleTestCase> {
  let RuleTrigger::Telemetry { key: trigger } = &model.when;
  if model.only_if.is_empty() {
    return Vec::new();
  }
  let mut keys: Vec<&str> = Vec::with_capacity(model.only_if.len());
  for c in &model.only_if {
    let (Some((key, _)), Some(_)) = (case_value(c, true), case_value(c, false)) else {
      return Vec::new();
    };
    if keys.contains(&key) {
      return Vec::new();
    }
    keys.push(key);
  }
  let hold = model.only_if.iter().map(hold_secs).max().unwrap_or(0);
  let any = model.r#match == RuleMatch::Any;
  // All: every row holds, or only the first does not. Any: only the first holds, or none does.
  let row_holds = |passes: bool, i: usize| {
    if any {
      passes && i == 0
    } else {
      passes || i > 0
    }
  };
  let case = |name: &str, passes: bool| {
    let mut metrics: Vec<RuleCaseMetric> = model
      .only_if
      .iter()
      .enumerate()
      .filter_map(|(i, c)| {
        let (key, value) = case_value(c, row_holds(passes, i))?;
        Some(RuleCaseMetric {
          key: key.to_string(),
          value: number_words(value),
        })
      })
      .collect();
    if !keys.contains(&trigger.as_str()) {
      metrics.push(RuleCaseMetric {
        key: trigger.clone(),
        value: "1".to_string(),
      });
    }
    let at = if hold > 0 {
      vec![0, i64::from(hold)]
    } else {
      vec![0]
    };
    let events: Vec<RuleCaseEvent> = at
      .iter()
      .map(|after| RuleCaseEvent {
        pigeon: RuleCasePigeon::A,
        readings: vec![RuleCaseReading {
          after_secs: *after,
          metrics: metrics.clone(),
        }],
      })
      .collect();
    // A row that holds only after its hold decides false on the first batch.
    let truths: Vec<bool> = at
      .iter()
      .map(|after| {
        let mut rows = model
          .only_if
          .iter()
          .enumerate()
          .map(|(i, c)| row_holds(passes, i) && *after >= i64::from(hold_secs(c)));
        if any {
          rows.any(|h| h)
        } else {
          rows.all(|h| h)
        }
      })
      .collect();
    let mut expect = expected_mails(&model.then, RuleBranch::Then, &truths);
    expect.extend(expected_mails(
      &model.otherwise,
      RuleBranch::Otherwise,
      &truths,
    ));
    RuleTestCase {
      name: name.to_string(),
      events,
      expect,
    }
  };
  vec![
    case("When the conditions are met", true),
    case("When they are not met", false),
  ]
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  /// The design's example: hot sign, held five minutes, a mail and a value, a fallback value.
  pub(crate) fn hot() -> RuleModel {
    serde_json::from_value(json!({
      "v": 1,
      "when": {"telemetry": {"key": "temp_c"}},
      "only_if": [
        {"is": "compare", "key": "temp_c", "op": "above", "to": {"number": 40}, "for_secs": 300}
      ],
      "then": [
        {"do": "mail", "severity": "warning", "subject": "Sign running hot: {pigeon} at {temp_c} C"},
        {"do": "set_value", "key": "fan", "value": "on"}
      ],
      "otherwise": [{"do": "set_value", "key": "fan", "value": "auto"}]
    }))
    .expect("the example parses")
  }

  #[test]
  fn the_example_round_trips_in_its_own_spelling() {
    let model = hot();
    assert_eq!(validate_rule_model(&model), Ok(()));
    let json = serde_json::to_value(&model).expect("serializes");
    assert_eq!(json["match"], "all");
    assert_eq!(json["only_if"][0]["is"], "compare");
    assert_eq!(json["then"][0]["repeat"], "start_stop");
    assert!(json["then"][0].get("body").is_none());
    let back: RuleModel = serde_json::from_value(json).expect("reads back");
    assert_eq!(back, model);
  }

  #[test]
  fn an_unknown_field_or_shape_is_refused_when_read() {
    for bad in [
      json!({"v": 1, "when": {"telemetry": {"key": "k"}}, "then": [], "extra": 1}),
      json!({"v": 1, "when": {"telemetry": {"key": "k", "x": 1}}}),
      json!({"v": 1, "when": {"schedule": {}}}),
      json!({"v": 1, "when": {"telemetry": {"key": "k"}},
        "only_if": [{"is": "compare", "key": "k", "op": "above", "to": {"number": 1}, "typo": 0}]}),
      json!({"v": 1, "when": {"telemetry": {"key": "k"}},
        "then": [{"do": "shadow", "key": "fan", "value": "on"}]}),
      json!({"v": 1, "when": {"telemetry": {"key": "k"}},
        "only_if": [{"is": "compare", "key": "k", "op": "above", "to": {"number": 1, "text": "x"}}]}),
    ] {
      assert!(
        serde_json::from_value::<RuleModel>(bad.clone()).is_err(),
        "{bad}"
      );
    }
  }

  fn with(edit: impl FnOnce(&mut RuleModel)) -> Result<(), RuleModelError> {
    let mut model = hot();
    edit(&mut model);
    validate_rule_model(&model)
  }

  fn problem(result: Result<(), RuleModelError>) -> RuleModelProblem {
    result.expect_err("refused").problem
  }

  #[test]
  fn every_key_is_bounded_and_none_is_proto() {
    let set_when = |k: &str| {
      let k = k.to_string();
      move |m: &mut RuleModel| m.when = RuleTrigger::Telemetry { key: k }
    };
    assert_eq!(problem(with(set_when(" "))), RuleModelProblem::EmptyKey);
    assert_eq!(
      problem(with(set_when(&"k".repeat(129)))),
      RuleModelProblem::KeyTooLong
    );
    assert_eq!(with(set_when(&"k".repeat(128))), Ok(()));
    assert_eq!(
      problem(with(set_when("a\nb"))),
      RuleModelProblem::KeyControl
    );
    let refused = with(set_when("__proto__")).expect_err("refused");
    assert_eq!(refused.place, RuleModelPlace::When);
    assert_eq!(refused.problem, RuleModelProblem::ReservedKey);
    let in_value = with(|m| {
      m.then[1] = RuleAction::SetValue {
        key: "__proto__".into(),
        value: "on".into(),
        bands: vec![],
      }
    });
    assert_eq!(
      in_value.expect_err("refused").place,
      RuleModelPlace::Action(RuleBranch::Then, 1)
    );
    let in_operand = with(|m| {
      m.only_if[0] = RuleCondition::Compare {
        key: "a".into(),
        op: RuleCompareOp::Above,
        to: RuleOperand::Key("__proto__".into()),
        for_secs: 0,
      }
    });
    assert_eq!(problem(in_operand), RuleModelProblem::ReservedKey);
  }

  #[test]
  fn conditions_are_checked_for_what_they_can_mean() {
    let row = |c: RuleCondition| move |m: &mut RuleModel| m.only_if = vec![c];
    let text_above = RuleCondition::Compare {
      key: "mode".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Text("night".into()),
      for_secs: 0,
    };
    assert_eq!(problem(with(row(text_above))), RuleModelProblem::TextOrder);
    let text_is = RuleCondition::Compare {
      key: "mode".into(),
      op: RuleCompareOp::NotEquals,
      to: RuleOperand::Text("night".into()),
      for_secs: 0,
    };
    assert_eq!(with(row(text_is)), Ok(()));
    let inverted = RuleCondition::Between {
      key: "lux".into(),
      low: 500.0,
      high: 100.0,
      for_secs: 0,
    };
    assert_eq!(problem(with(row(inverted))), RuleModelProblem::LowAboveHigh);
    let infinite = RuleCondition::Compare {
      key: "t".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(f64::INFINITY),
      for_secs: 0,
    };
    assert_eq!(problem(with(row(infinite))), RuleModelProblem::NotFinite);
    let bit = RuleCondition::BitSet {
      key: "reset_cause".into(),
      bit: 31,
    };
    assert_eq!(problem(with(row(bit))), RuleModelProblem::BitRange);
    let negative = RuleCondition::ChangedBy {
      key: "t".into(),
      more_than: -1.0,
    };
    assert_eq!(problem(with(row(negative))), RuleModelProblem::Negative);
    let count = |more_than, within_secs| RuleCondition::Counted {
      key: "reset_cause".into(),
      more_than,
      within_secs,
    };
    assert_eq!(with(row(count(3, 86_400))), Ok(()));
    assert_eq!(
      problem(with(row(count(32, 86_400)))),
      RuleModelProblem::CountTooHigh
    );
    assert_eq!(
      problem(with(row(count(3, 59)))),
      RuleModelProblem::WindowRange
    );
    assert_eq!(
      problem(with(row(count(3, MAX_MODEL_WINDOW_SECS + 1)))),
      RuleModelProblem::WindowRange
    );
    let long_hold = RuleCondition::Between {
      key: "lux".into(),
      low: 1.0,
      high: 2.0,
      for_secs: MAX_MODEL_WINDOW_SECS + 1,
    };
    assert_eq!(problem(with(row(long_hold))), RuleModelProblem::WindowRange);
  }

  #[test]
  fn the_caps_bound_rows_mails_counts_and_bytes() {
    let compare = hot().only_if[0].clone();
    assert_eq!(
      problem(with(|m| m.only_if = vec![compare.clone(); 17])),
      RuleModelProblem::TooManyConditions
    );
    assert_eq!(with(|m| m.only_if = vec![compare.clone(); 16]), Ok(()));
    let counted = RuleCondition::Counted {
      key: "reset_cause".into(),
      more_than: 3,
      within_secs: 3_600,
    };
    let refused = with(|m| m.only_if = vec![counted; 5]).expect_err("refused");
    assert_eq!(refused.place, RuleModelPlace::Condition(4));
    assert_eq!(refused.problem, RuleModelProblem::TooManyCounted);
    let mail = hot().then[0].clone();
    assert_eq!(
      problem(with(|m| {
        m.then = vec![mail.clone(); 3];
        m.otherwise = vec![mail.clone(); 2];
      })),
      RuleModelProblem::TooManyMails
    );
    let value = hot().then[1].clone();
    assert_eq!(
      problem(with(|m| m.otherwise = vec![value.clone(); 9])),
      RuleModelProblem::TooManyActions
    );
    assert_eq!(
      problem(with(|m| {
        m.then.clear();
        m.otherwise.clear();
      })),
      RuleModelProblem::NoAction
    );
    let huge = "x".repeat(1_000);
    assert_eq!(
      problem(with(|m| {
        m.only_if = (0..16)
          .map(|i| RuleCondition::Compare {
            key: i.to_string(),
            op: RuleCompareOp::Equals,
            to: RuleOperand::Text(huge.clone()),
            for_secs: 0,
          })
          .collect();
      })),
      RuleModelProblem::TooLarge
    );
  }

  #[test]
  fn mail_and_value_texts_keep_the_result_caps() {
    let mail = |subject: &str, body: &str| {
      let (subject, body) = (subject.to_string(), body.to_string());
      move |m: &mut RuleModel| {
        m.then[0] = RuleAction::Mail {
          severity: RuleSeverity::Info,
          subject,
          body,
          repeat: RuleMailRepeat::Once,
        }
      }
    };
    assert_eq!(problem(with(mail(" ", ""))), RuleModelProblem::EmptySubject);
    assert_eq!(
      problem(with(mail(&"s".repeat(121), ""))),
      RuleModelProblem::SubjectTooLong
    );
    assert_eq!(
      problem(with(mail("a\nb", ""))),
      RuleModelProblem::TextControl
    );
    assert_eq!(with(mail("ok", "line one\nline two")), Ok(()));
    assert_eq!(
      problem(with(mail("ok", "tab\there"))),
      RuleModelProblem::TextControl
    );
    assert_eq!(
      problem(with(mail("ok", &"b".repeat(1_025)))),
      RuleModelProblem::BodyTooLong
    );
    let value = |value: &str| {
      let value = value.to_string();
      move |m: &mut RuleModel| {
        m.then[1] = RuleAction::SetValue {
          key: "fan".into(),
          value,
          bands: vec![],
        }
      }
    };
    assert_eq!(
      problem(with(value(&"v".repeat(1_025)))),
      RuleModelProblem::TextTooLong
    );
    assert_eq!(problem(with(value("a\0b"))), RuleModelProblem::TextControl);
  }

  #[test]
  fn a_value_table_row_keeps_no_state() {
    let band = |when: RuleCondition| {
      move |m: &mut RuleModel| {
        m.then[1] = RuleAction::SetValue {
          key: "brightness".into(),
          value: "60".into(),
          bands: vec![RuleBand {
            when,
            value: "80".into(),
          }],
        }
      }
    };
    let plain = RuleCondition::Compare {
      key: "lux".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(500.0),
      for_secs: 0,
    };
    assert_eq!(with(band(plain)), Ok(()));
    let held = RuleCondition::Compare {
      key: "lux".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(500.0),
      for_secs: 60,
    };
    let refused = with(band(held)).expect_err("refused");
    assert_eq!(refused.place, RuleModelPlace::Band(RuleBranch::Then, 1, 0));
    assert_eq!(refused.problem, RuleModelProblem::StatefulBand);
    assert_eq!(
      refused.to_string(),
      "Then, row 2, table row 1: a table row cannot wait or count"
    );
  }

  #[test]
  fn a_newer_model_is_refused_by_name() {
    let refused = with(|m| m.v = 2).expect_err("refused");
    assert_eq!(
      refused.to_string(),
      "The rule: this rule was made by a newer builder"
    );
  }

  #[test]
  fn the_module_parses_its_model_from_a_string() {
    let mut model = hot();
    model.then[0] = RuleAction::Mail {
      severity: RuleSeverity::Warning,
      subject: "\"quoted\" \\ and \u{2028} line".into(),
      body: String::new(),
      repeat: RuleMailRepeat::StartStop,
    };
    let source = compile_rule_model(&model);
    assert_eq!(source.len(), source.capacity());
    assert!(source.starts_with(MODULE_HEAD));
    assert!(source.ends_with(MODULE_TAIL));
    assert_eq!(crate::check_rule_source(&source), Ok(()));
    let literal = &source[MODULE_HEAD.len()..source.len() - MODULE_TAIL.len()];
    let json: String = serde_json::from_str(literal).expect("a JSON string literal");
    let back: RuleModel = serde_json::from_str(&json).expect("the model");
    assert_eq!(back, model);
    assert_eq!(
      source.lines().filter(|l| l.starts_with("import")).count(),
      1
    );
  }

  #[test]
  fn mail_keys_name_their_action() {
    let model = hot();
    assert_eq!(mail_alert_key(RuleBranch::Then, 0), "then-0");
    assert_eq!(mail_alert_key(RuleBranch::Otherwise, 3), "else-3");
    let (branch, action) = mail_action_for_key(&model, "then-0").expect("a mail");
    assert_eq!(branch, RuleBranch::Then);
    assert!(matches!(action, RuleAction::Mail { .. }));
    assert!(mail_action_for_key(&model, "then-0-4").is_some());
    assert!(
      mail_action_for_key(&model, "then-1").is_none(),
      "a value, not a mail"
    );
    assert!(mail_action_for_key(&model, "else-0").is_none());
    assert!(mail_action_for_key(&model, "too-hot").is_none());
  }

  #[test]
  fn the_sentence_reads_the_rule_back() {
    assert_eq!(
      rule_model_sentence(&hot()),
      "When a pigeon reports temp_c, if temp_c is above 40 for 5 minutes: mail \u{201c}Sign \
       running hot: {pigeon} at {temp_c} C\u{201d} (worth a look), when this starts and when \
       it stops; and set fan to \u{201c}on\u{201d}. Otherwise: set fan to \u{201c}auto\u{201d}."
    );
    let mut any = hot();
    any.r#match = RuleMatch::Any;
    any.only_if.push(RuleCondition::Counted {
      key: "reset_cause".into(),
      more_than: 1,
      within_secs: 86_400,
    });
    any.otherwise.clear();
    any.then = vec![RuleAction::SetValue {
      key: "brightness".into(),
      value: "60".into(),
      bands: vec![RuleBand {
        when: RuleCondition::Between {
          key: "lux".into(),
          low: 0.5,
          high: 500.0,
          for_secs: 0,
        },
        value: "30".into(),
      }],
    }];
    assert_eq!(
      rule_model_sentence(&any),
      "When a pigeon reports temp_c, if temp_c is above 40 for 5 minutes or reset_cause is \
       reported more than 1 time within 1 day: set brightness to \u{201c}30\u{201d} if lux is \
       between 0.5 and 500, else \u{201c}60\u{201d}."
    );
    assert_eq!(duration_words(90), "90 seconds");
    assert_eq!(duration_words(7_200), "2 hours");
    assert_eq!(duration_words(60), "1 minute");
  }

  #[test]
  fn test_cases_come_from_number_rows_with_their_holds() {
    let cases = rule_model_test_cases(&hot());
    assert_eq!(cases.len(), 2);
    let met = &cases[0];
    assert_eq!(met.events.len(), 2, "a hold adds a later batch");
    assert_eq!(met.events[1].readings[0].after_secs, 300);
    assert_eq!(met.events[0].readings[0].metrics[0].key, "temp_c");
    assert_eq!(met.events[0].readings[0].metrics[0].value, "41");
    assert_eq!(
      met.expect,
      vec![RuleCaseMail {
        key: "then-0".into(),
        kind: RuleAlertKind::Fire
      }]
    );
    let unmet = &cases[1];
    assert_eq!(unmet.events[0].readings[0].metrics[0].value, "40");
    assert!(
      unmet.expect.is_empty(),
      "a clear without a fire mails nothing"
    );
  }

  #[test]
  fn test_cases_follow_any_and_otherwise() {
    let mail = |repeat| RuleAction::Mail {
      severity: RuleSeverity::Critical,
      subject: "s".into(),
      body: String::new(),
      repeat,
    };
    let model = RuleModel {
      v: 1,
      when: RuleTrigger::Telemetry {
        key: "uptime_s".into(),
      },
      r#match: RuleMatch::Any,
      only_if: vec![
        RuleCondition::Compare {
          key: "lux".into(),
          op: RuleCompareOp::AtMost,
          to: RuleOperand::Number(10.0),
          for_secs: 0,
        },
        RuleCondition::Between {
          key: "temp_c".into(),
          low: -5.0,
          high: 5.0,
          for_secs: 0,
        },
      ],
      then: vec![mail(RuleMailRepeat::Once)],
      otherwise: vec![mail(RuleMailRepeat::StartStop)],
    };
    let cases = rule_model_test_cases(&model);
    let metrics = |case: &RuleTestCase| {
      case.events[0].readings[0]
        .metrics
        .iter()
        .map(|m| [m.key.as_str(), "=", &m.value].concat())
        .collect::<Vec<_>>()
    };
    assert_eq!(metrics(&cases[0]), ["lux=10", "temp_c=6", "uptime_s=1"]);
    assert_eq!(metrics(&cases[1]), ["lux=11", "temp_c=6", "uptime_s=1"]);
    assert_eq!(cases[0].expect[0].key, "then-0-1");
    assert_eq!(cases[0].expect[0].kind, RuleAlertKind::Notify);
    assert_eq!(
      cases[1].expect,
      vec![RuleCaseMail {
        key: "else-0".into(),
        kind: RuleAlertKind::Fire
      }]
    );
  }

  #[test]
  fn no_cases_for_rows_a_case_cannot_spell() {
    let mut model = hot();
    model.only_if = vec![RuleCondition::Changed {
      key: "layout".into(),
    }];
    assert!(rule_model_test_cases(&model).is_empty());
    model.only_if.clear();
    assert!(rule_model_test_cases(&model).is_empty());
    let twice = hot().only_if[0].clone();
    model.only_if = vec![twice.clone(), twice];
    assert!(
      rule_model_test_cases(&model).is_empty(),
      "two rows on one key"
    );
  }

  #[test]
  fn case_values_stay_past_thresholds_beyond_integer_precision() {
    let value = |case: &RuleTestCase| {
      case.events[0].readings[0].metrics[0]
        .value
        .parse::<f64>()
        .expect("a number")
    };
    let mut model = hot();
    model.only_if = vec![RuleCondition::Compare {
      key: "a".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(1e17),
      for_secs: 0,
    }];
    let cases = rule_model_test_cases(&model);
    assert!(value(&cases[0]) > 1e17, "the met case is above");
    assert_eq!(value(&cases[1]), 1e17);
    model.only_if = vec![RuleCondition::Between {
      key: "a".into(),
      low: -1e300,
      high: 1e300,
      for_secs: 0,
    }];
    let cases = rule_model_test_cases(&model);
    assert!(value(&cases[1]) > 1e300, "the unmet case is past high");
    model.only_if = vec![RuleCondition::Compare {
      key: "a".into(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(f64::MAX),
      for_secs: 0,
    }];
    assert!(
      rule_model_test_cases(&model).is_empty(),
      "no finite number is above it"
    );
  }
}
