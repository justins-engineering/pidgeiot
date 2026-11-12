//! The pure core of user-authored rules: from one telemetry batch to the statements and mails its
//! rules cause, with no I/O. A dispatch reaches this module as data, the status and body the rules
//! script answered or the way the call failed, so every outcome the namespace can produce is a
//! plain `cargo test -p dovecote` case.
//!
//! Nothing a rules script returns is trusted. The harness shares an isolate with user code, so
//! every body is read under a cap, every result re-validated by [`validate_result`], and anything
//! that is not the harness's own shape is the flock's failure.
#![cfg_attr(
  not(test),
  expect(
    dead_code,
    reason = "its callers are the queue's second pass and the rule routes"
  )
)]

use crate::helpers::{ResolvedReading, sha256_hex};
use capsules::{
  MAX_RULE_ERROR_BYTES, MAX_RULE_LOG_LINE_BYTES, MAX_RULE_LOG_LINES, MAX_RULE_STATE_TOTAL_BYTES,
  MAX_TELEMETRY_BATCH_READINGS, MAX_TELEMETRY_KEY_BYTES, MAX_TELEMETRY_KEYS,
  MAX_TELEMETRY_VALUE_BYTES, RULE_CONTRACT, RuleAlert, RuleAlertKind, RuleAlertScope,
  RuleCaseMetric, RuleCasePigeon, RuleCaseReading, RuleHook, RuleNamedRef, RulePrevious,
  RuleReading, RuleResult, RuleSeverity, RuleState, RuleTestCase, TelemetryHistoryPoint,
  merge_state, state_doc_bytes, validate_result,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

/// The production pigeons of signs 414, 73 and 1670, by full id. No rule reads or writes under
/// them until the owner rules otherwise, and they are matched whole, never by prefix.
pub const FIELD_PIGEONS: [&str; 3] = [
  "fa8a80551f1818392b2929f8dc4cf0efb05155e7b7316692ce944eae6ce7a33f",
  "ae3a6d3f63a5231601c416510612fa330d9339ccac86427c6adb93321dd3edc7",
  "c8bd5d567beef36d185bcbf6e8625c2c8a6a755d387ae8a513ee88d779c299f2",
];

/// One budget of wall time for a whole live dispatch, all rules together, kept by the harness.
pub const HARNESS_WALL_MS: u32 = 250;

/// CPU per live dispatch, shared by the bundle. Only the upload metadata binds it: a per-call
/// `cpuMs` is ignored. The platform's kill lands anywhere from the limit to about 2 s past it,
/// so a dispatch can cost about 2.05 s of CPU, and a kill that late reaches dovecote as the 2 s
/// race's loss. Over-limit work that finishes unkilled is an `ok` run the auto-pause never sees,
/// so that per-dispatch bound applies indefinitely.
pub const RULE_CPU_MS: u32 = 50;

/// CPU for a dry run's test script, which replays days of telemetry in one call. With the
/// platform's grace (see [`RULE_CPU_MS`]) a call can spend about 4 s, if the grace measured only
/// up to a 200 ms limit holds at 2000.
pub const TEST_RULE_CPU_MS: u32 = 2_000;

/// The most one rule's entry takes in a response: its result escaped twice, its logs, framing
/// (`dovecote/rules/wire.d.ts`).
pub const RULE_WIRE_BYTES: usize = 103 * 1024;

/// No new dispatch starts this long after the second pass began, bounding the consumer slot and
/// Hyperdrive time the pass holds.
pub const PASS_BUDGET_MS: u64 = 3_000;

/// An answered dispatch slower than this skips its account's other flocks for the invocation, as
/// a failed one does. The harness stops a rule at [`HARNESS_WALL_MS`], so the rest is CPU it could
/// not time, and two such flocks would otherwise spend the whole [`PASS_BUDGET_MS`].
pub const SLOW_DISPATCH_MS: u64 = 4 * HARNESS_WALL_MS as u64;

/// Consecutive failing runs that auto-pause a rule.
pub const AUTO_PAUSE_RUNS: i32 = 20;

/// How old a `dispatch_failed` streak must also be before it pauses, so a runtime outage shorter
/// than this pauses nothing while a rule that breaks every dispatch stops within it. A graced CPU
/// kill arrives as a lost race, so it waits out this floor too.
pub const DISPATCH_FAILED_PAUSE_SECS: i64 = 3_600;

/// A version mismatch is a deploy in flight until it has lasted this long, then a run row.
pub const MISMATCH_ROW_AFTER_SECS: i64 = 600;

/// The least time between two changes of one flock-scope flag, so batches decided on different
/// snapshots cannot toggle a fleet mail.
pub const FLOCK_FLAG_HOLD_SECS: i64 = 600;

/// A `notify` key mails at most once in this window.
pub const NOTIFY_REPEAT_SECS: i64 = 7 * 86_400;

/// Rule mail caps: per rule an hour and a day, per account a day, platform-wide a day.
pub const RULE_MAIL_PER_HOUR: i32 = 10;
/// See [`RULE_MAIL_PER_HOUR`].
pub const RULE_MAIL_PER_DAY: i32 = 50;
/// See [`RULE_MAIL_PER_HOUR`].
pub const ACCOUNT_RULE_MAIL_PER_DAY: i32 = 200;
/// See [`RULE_MAIL_PER_HOUR`].
pub const PLATFORM_RULE_MAIL_PER_DAY: i32 = 2_000;

/// Most readings one dry run replays, newest kept.
pub const REPLAY_MAX_READINGS: usize = 4_000;

/// The compatibility date every rules script is uploaded with, moved only deliberately. It is
/// kept apart from dovecote's own date so a dovecote bump does not change the runtime of rules
/// tested under this one. `rules/run-workerd.mjs` and `rules-outbound/test.mjs` read this
/// declaration's text, so it stays one `&str` literal; the latter holds the outbound Worker's
/// `wrangler.toml` to it.
pub const RULES_COMPATIBILITY_DATE: &str = "2026-10-26";

/// Bounds on a dry run's synthetic test cases, which a model's structured output does not enforce.
pub const MIN_TEST_CASES: usize = 2;
/// See [`MIN_TEST_CASES`].
pub const MAX_TEST_CASES: usize = 5;
/// See [`MIN_TEST_CASES`].
pub const MAX_CASE_EVENTS: usize = 8;
/// Furthest a case reading may sit from the case's start: the notify window.
pub const MAX_CASE_OFFSET_SECS: i64 = NOTIFY_REPEAT_SECS;

/// The harness modules every bundle carries beside its rules, the builder's interpreter among
/// them so a builder rule costs its three lines and no more.
const HARNESS: [(&str, &str); 4] = [
  ("main.js", include_str!("../../rules/main.js")),
  ("prelude.js", include_str!("../../rules/prelude.js")),
  ("pidgeiot.js", include_str!("../../rules/pidgeiot.js")),
  ("builder.js", include_str!("../../rules/builder.js")),
];

fn is_pigeon_id(s: &str) -> bool {
  s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn is_lower_hex(s: &str) -> bool {
  s.bytes()
    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Every pigeon id rules must never touch: [`FIELD_PIGEONS`] plus each 64-hex id in the
/// comma-separated `RULES_EXCLUDED_PIGEONS` var. The var can only add: an unset, empty or
/// malformed var still excludes the three, and a malformed entry is dropped.
pub fn excluded_pigeons(var: Option<&str>) -> Vec<String> {
  let mut ids: Vec<String> = FIELD_PIGEONS.iter().map(|id| (*id).to_string()).collect();
  for entry in var.unwrap_or("").split(',') {
    let entry = entry.trim().to_ascii_lowercase();
    if is_pigeon_id(&entry) && !ids.contains(&entry) {
      ids.push(entry);
    }
  }
  ids
}

/// What happens to one batch before any statement runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreGate {
  /// `RULES_DISPATCH` is not `"on"`: rules do not run anywhere.
  Off,
  /// The pigeon is excluded: nothing is read or written under its id.
  Excluded,
  /// Go on to the cached gate; the only answer after which a statement runs.
  Lookup,
}

fn dispatch_on(dispatch: Option<&str>) -> bool {
  dispatch.map(str::trim) == Some("on")
}

/// The second pass's vars, read once per consumer invocation. One exists only when
/// `RULES_DISPATCH` is `"on"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassVars {
  /// [`excluded_pigeons`] of `RULES_EXCLUDED_PIGEONS`, parsed once for the whole pass.
  pub excluded: Vec<String>,
  /// The raw `RULES_ALLOWED_ORG_IDS`: a flock of an organization it does not list is not
  /// dispatched, so narrowing it stops deployed rules as well as their authors.
  pub allowed_orgs: Option<String>,
}

impl PassVars {
  /// [`pre_gate`] for one batch with the switch known to be on.
  pub fn gate(&self, pigeon_id: &str) -> PreGate {
    pre_gate(pigeon_id, Some("on"), &self.excluded)
  }
}

/// Reads `RULES_DISPATCH` through `read` and, only when it is `"on"`, `RULES_EXCLUDED_PIGEONS`
/// and `RULES_ALLOWED_ORG_IDS`.
/// `None` means rules are off and nothing else was read, so an unset switch costs the consumer
/// no binding, statement or dispatch.
pub fn pass_vars(read: impl Fn(&str) -> Option<String>) -> Option<PassVars> {
  let dispatch = read("RULES_DISPATCH")?;
  if !dispatch_on(Some(&dispatch)) {
    return None;
  }
  Some(PassVars {
    excluded: excluded_pigeons(read("RULES_EXCLUDED_PIGEONS").as_deref()),
    allowed_orgs: read("RULES_ALLOWED_ORG_IDS"),
  })
}

/// Decides [`PreGate`] from the switch and the [`excluded_pigeons`] list alone, so an excluded
/// pigeon costs no statement.
pub fn pre_gate(pigeon_id: &str, dispatch: Option<&str>, excluded: &[String]) -> PreGate {
  if !dispatch_on(dispatch) {
    return PreGate::Off;
  }
  if excluded.iter().any(|id| id.eq_ignore_ascii_case(pigeon_id)) {
    return PreGate::Excluded;
  }
  PreGate::Lookup
}

/// The event's `batch_id`: sha256 over the pigeon id and the queue message's own payload string,
/// the exact bytes enqueued, so a redelivery carries the same id with no producer change.
pub fn batch_id(pigeon_id: &str, payload: &str) -> String {
  let mut input = String::with_capacity(pigeon_id.len() + 1 + payload.len());
  input.push_str(pigeon_id);
  input.push('\n');
  input.push_str(payload);
  sha256_hex(input.as_bytes())
}

/// One pigeon's batch waiting for the second pass. It owns no endpoint, connector or token, so
/// none can reach an event.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleWork {
  /// The pigeon whose batch this is.
  pub pigeon_id: String,
  /// See [`batch_id`].
  pub batch_id: String,
  /// True when the Durable Object's result failed to parse and `previous` was lost.
  pub degraded: bool,
  /// The batch, as the pipeline resolved it.
  pub readings: Vec<ResolvedReading>,
}

impl RuleWork {
  /// `payload` is the message's `readings_json`, or `metrics_json` for a legacy message. `None`
  /// for a batch with no readings, such as a seen-only message, which runs no rule.
  pub fn new(
    pigeon_id: &str,
    payload: &str,
    degraded: bool,
    readings: Vec<ResolvedReading>,
  ) -> Option<Self> {
    if readings.is_empty() {
      return None;
    }
    Some(Self {
      pigeon_id: pigeon_id.to_string(),
      batch_id: batch_id(pigeon_id, payload),
      degraded,
      readings,
    })
  }
}

/// The event without its per-rule `rule` and `state`, shared by every rule of one dispatch.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct CommonEvent {
  /// Always [`RULE_CONTRACT`].
  pub contract: u32,
  /// Always [`RuleHook::Telemetry`] in contract 1.
  pub hook: RuleHook,
  /// See [`batch_id`].
  pub batch_id: String,
  /// Unix seconds when the event was built; a replay's last reading.
  pub received_at: i64,
  /// See [`RuleWork::degraded`].
  pub degraded: bool,
  /// The flock, id and name only.
  pub flock: RuleNamedRef,
  /// The pigeon, id and name only.
  pub pigeon: RuleNamedRef,
  /// Oldest first.
  pub readings: Vec<RuleReading>,
}

/// The pipeline's readings in the contract's spelling, oldest first; `previous` is `{}` when the
/// batch is degraded or nothing was displaced.
pub fn rule_readings(readings: &[ResolvedReading], degraded: bool) -> Vec<RuleReading> {
  let mut out: Vec<RuleReading> = readings
    .iter()
    .map(|r| RuleReading {
      at: r.at_secs,
      metrics: r
        .metrics
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect(),
      previous: match (&r.previous, degraded) {
        (Some(previous), false) => previous
          .iter()
          .map(|(k, p)| {
            let entry = RulePrevious {
              value: p.value.clone(),
              at: p.reported_at,
            };
            (k.clone(), entry)
          })
          .collect(),
        _ => BTreeMap::new(),
      },
    })
    .collect();
  out.sort_by_key(|r| r.at);
  out
}

/// Builds the shared event from named fields only, which is what keeps credentials out of it.
pub fn build_event(
  work: &RuleWork,
  flock: RuleNamedRef,
  pigeon: RuleNamedRef,
  received_at: i64,
) -> CommonEvent {
  CommonEvent {
    contract: RULE_CONTRACT,
    hook: RuleHook::Telemetry,
    batch_id: work.batch_id.clone(),
    received_at,
    degraded: work.degraded,
    flock,
    pigeon,
    readings: rule_readings(&work.readings, work.degraded),
  }
}

/// One enabled rule as a dispatch names it, with the state the fresh read found.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct DispatchRule {
  /// The rule's id, which is also its key in the bundle's `rules.js`.
  pub id: String,
  /// Its deployed version; the harness answers `version_mismatch` when the bundle holds another.
  pub version: i32,
  /// Its two documents.
  pub state: RuleState,
}

/// `LiveInput` in `dovecote/rules/wire.d.ts`.
#[derive(Serialize)]
struct LiveInput<'a> {
  mode: &'static str,
  wall_ms: u32,
  common: &'a CommonEvent,
  rules: &'a [DispatchRule],
}

/// The request body of a live dispatch (`LiveInput` in `wire.d.ts`).
pub fn live_input(common: &CommonEvent, rules: &[DispatchRule]) -> Option<String> {
  let input = LiveInput {
    mode: "live",
    wall_ms: HARNESS_WALL_MS,
    common,
    rules,
  };
  serde_json::to_string(&input).ok()
}

/// The state one dispatch carries: every rule's pigeon and flock documents, as the
/// [`MAX_RULE_STATE_TOTAL_BYTES`] budget counts them.
pub fn state_total(rules: &[DispatchRule]) -> usize {
  rules.iter().fold(0, |total, rule| {
    total
      .saturating_add(state_doc_bytes(&rule.state.pigeon))
      .saturating_add(state_doc_bytes(&rule.state.flock))
  })
}

/// Most bytes of a response read for a dispatch of `rules` rules; one more is `oversize`.
pub fn read_cap(rules: usize) -> usize {
  rules.saturating_mul(RULE_WIRE_BYTES).saturating_add(1024)
}

/// How a dispatch ended, before anything of it is believed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
  /// The namespace binding was missing or had no `get`: the platform's failure.
  Binding,
  /// The call rejected or lost the 2 s race. Its message is never read: a rule's own throw
  /// crosses the binding with its text intact, so a CPU or memory kill, a missing script and a
  /// rule forging any of their messages are told apart by nothing a rule cannot write.
  Failed,
  /// The script answered. `body` holds at most [`read_cap`] + 1 bytes.
  Answered {
    /// HTTP status.
    status: u16,
    /// The body as read.
    body: Vec<u8>,
  },
}

/// A rule run's outcome, as `rule_runs.status` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
  /// A valid result.
  Ok,
  /// The rule threw.
  Threw,
  /// The harness's wall budget ran out in this rule.
  Timeout,
  /// An earlier rule spent the wall budget.
  Skipped,
  /// The bundle holds another version: a deploy in flight.
  VersionMismatch,
  /// A result the harness or the validator refused.
  BadOutput,
  /// The response was over [`read_cap`].
  Oversize,
  /// The dispatch failed in a way user code can cause: a 5xx, a harness failure, a lost race, a
  /// rejection, or a body that is not the harness's.
  DispatchFailed,
}

impl RunStatus {
  /// The status as stored and shown.
  pub fn as_str(self) -> &'static str {
    match self {
      RunStatus::Ok => "ok",
      RunStatus::Threw => "threw",
      RunStatus::Timeout => "timeout",
      RunStatus::Skipped => "skipped",
      RunStatus::VersionMismatch => "version_mismatch",
      RunStatus::BadOutput => "bad_output",
      RunStatus::Oversize => "oversize",
      RunStatus::DispatchFailed => "dispatch_failed",
    }
  }

  /// Whether the run extends the rule's failure streak.
  pub fn counts_toward_pause(self) -> bool {
    !matches!(
      self,
      RunStatus::Ok | RunStatus::Skipped | RunStatus::VersionMismatch
    )
  }
}

/// One rule's run, re-read from the response.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
  /// The rule's id.
  pub id: String,
  /// Its outcome.
  pub status: RunStatus,
  /// The validated result, only when `Ok`.
  pub result: Option<RuleResult>,
  /// User code's own text or a refusal, at most [`MAX_RULE_ERROR_BYTES`].
  pub error: Option<String>,
  /// Captured `console.log` lines, within [`MAX_RULE_LOG_LINES`] of [`MAX_RULE_LOG_LINE_BYTES`].
  pub logs: Vec<String>,
}

/// A dispatch, attributed.
#[derive(Debug, Clone, PartialEq)]
pub enum Classified {
  /// The harness answered for each rule, in the order sent.
  PerRule(Vec<RunRecord>),
  /// The dispatch failed whole: every rule records this status and the flock's account is skipped
  /// for the rest of the invocation.
  Flock(RunStatus),
  /// The platform failed: nothing is counted against anyone and the breaker trips.
  Platform,
}

/// `LiveOutput` in `dovecote/rules/wire.d.ts`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireOutput {
  contract: u32,
  #[serde(default)]
  results: Option<Vec<WireRun>>,
  #[serde(default)]
  error: Option<String>,
}

/// One entry of `LiveOutput.results` in `dovecote/rules/wire.d.ts`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRun {
  id: String,
  status: String,
  #[serde(default)]
  out_json: Option<String>,
  #[serde(default)]
  error: Option<String>,
  #[serde(default)]
  logs: Vec<String>,
}

/// Reads one dispatch's outcome against the rules it named.
///
/// The response must be the harness's own shape, one result per rule sent and in order; anything
/// else is the flock's `dispatch_failed`, since user code can rewrite the body. Each `ok` result
/// is validated alone against that rule's state.
pub fn classify(sent: &[DispatchRule], outcome: DispatchOutcome) -> Classified {
  let body = match outcome {
    DispatchOutcome::Binding => return Classified::Platform,
    DispatchOutcome::Failed => return Classified::Flock(RunStatus::DispatchFailed),
    DispatchOutcome::Answered { status: 200, body } => body,
    DispatchOutcome::Answered { .. } => return Classified::Flock(RunStatus::DispatchFailed),
  };
  if body.len() > read_cap(sent.len()) {
    return Classified::Flock(RunStatus::Oversize);
  }
  let Ok(output) = serde_json::from_slice::<WireOutput>(&body) else {
    return Classified::Flock(RunStatus::DispatchFailed);
  };
  let runs = match output {
    WireOutput {
      contract: RULE_CONTRACT,
      results: Some(runs),
      error: None,
    } if runs.len() == sent.len() => runs,
    _ => return Classified::Flock(RunStatus::DispatchFailed),
  };
  let mut records = Vec::with_capacity(sent.len());
  for (rule, run) in sent.iter().zip(runs) {
    let Some(record) = run_record(rule, run) else {
      return Classified::Flock(RunStatus::DispatchFailed);
    };
    records.push(record);
  }
  Classified::PerRule(records)
}

fn run_record(rule: &DispatchRule, run: WireRun) -> Option<RunRecord> {
  if run.id != rule.id {
    return None;
  }
  let logs = run
    .logs
    .iter()
    .take(MAX_RULE_LOG_LINES)
    .map(|line| cut(&without_nul(line), MAX_RULE_LOG_LINE_BYTES))
    .collect();
  let error = run
    .error
    .as_deref()
    .map(|e| cut(&without_nul(e), MAX_RULE_ERROR_BYTES));
  let (status, result, error) = match run.status.as_str() {
    "ok" => match validate_result(run.out_json.as_deref()?, &rule.state) {
      Ok(result) => (RunStatus::Ok, Some(result), None),
      Err(violation) => (RunStatus::BadOutput, None, Some(violation.to_string())),
    },
    "threw" => (RunStatus::Threw, None, error),
    "timeout" => (RunStatus::Timeout, None, None),
    "skipped" => (RunStatus::Skipped, None, None),
    "version_mismatch" => (RunStatus::VersionMismatch, None, None),
    "bad_output" => (RunStatus::BadOutput, None, error),
    _ => return None,
  };
  Some(RunRecord {
    id: run.id,
    status,
    result,
    error,
    logs,
  })
}

/// `s` with each U+0000 replaced by U+FFFD: Postgres refuses U+0000 in `text` and `jsonb`, and a
/// refused run row would fail the rule's whole apply.
pub(crate) fn without_nul(s: &str) -> std::borrow::Cow<'_, str> {
  if s.contains('\0') {
    s.replace('\0', "\u{fffd}").into()
  } else {
    s.into()
  }
}

/// `s` cut to at most `max` bytes on a character boundary.
pub(crate) fn cut(s: &str, max: usize) -> String {
  let mut end = s.len().min(max);
  while !s.is_char_boundary(end) {
    end -= 1;
  }
  s[..end].to_string()
}

/// Why the second pass did not dispatch a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
  /// The breaker tripped this invocation.
  Breaker,
  /// A dispatch of this account's already failed, or answered slowly, this invocation.
  AccountFailed,
  /// [`PASS_BUDGET_MS`] is spent.
  Budget,
}

/// One consumer invocation's second pass: which accounts are skipped, which of them failed,
/// whether the breaker tripped, and when the pass started. Times are milliseconds from any fixed
/// origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassLedger {
  started_ms: u64,
  skipped_accounts: Vec<String>,
  failed_accounts: Vec<String>,
  tripped: bool,
}

impl PassLedger {
  /// A pass starting at `started_ms`.
  pub fn new(started_ms: u64) -> Self {
    Self {
      started_ms,
      skipped_accounts: Vec::new(),
      failed_accounts: Vec::new(),
      tripped: false,
    }
  }

  /// Whether a batch of a flock `account` owns may dispatch at `now_ms`.
  pub fn may_dispatch(&self, account: &str, now_ms: u64) -> Result<(), Skip> {
    if self.tripped {
      Err(Skip::Breaker)
    } else if self.skipped_accounts.iter().any(|a| a == account) {
      Err(Skip::AccountFailed)
    } else if self.budget_spent(now_ms) {
      Err(Skip::Budget)
    } else {
      Ok(())
    }
  }

  /// Whether the breaker tripped, after which nothing else in the pass runs a statement.
  pub fn tripped(&self) -> bool {
    self.tripped
  }

  /// Whether [`PASS_BUDGET_MS`] has passed by `now_ms`, checked before a batch's first statement.
  pub fn budget_spent(&self, now_ms: u64) -> bool {
    now_ms.saturating_sub(self.started_ms) >= PASS_BUDGET_MS
  }

  /// Records a dispatch of a flock `account` owns that took `wall_ms`. A failure, or an answer
  /// slower than [`SLOW_DISPATCH_MS`], skips that account's flocks for the rest of the pass, so it
  /// costs others at most one 2 s wait. A second account's failure, or a platform failure, trips
  /// the breaker, so one tenant cannot trip it alone; a slow answer never counts toward it.
  pub fn record(&mut self, account: &str, classified: &Classified, wall_ms: u64) {
    let skip = match classified {
      Classified::Platform => {
        self.tripped = true;
        false
      }
      Classified::Flock(_) => {
        if !self.failed_accounts.iter().any(|a| a == account) {
          self.failed_accounts.push(account.to_string());
        }
        if self.failed_accounts.len() >= 2 {
          self.tripped = true;
        }
        true
      }
      Classified::PerRule(_) => wall_ms >= SLOW_DISPATCH_MS,
    };
    if skip && !self.skipped_accounts.iter().any(|a| a == account) {
      self.skipped_accounts.push(account.to_string());
    }
  }
}

/// A rule's failure streak, as `rules.fail_count` and `rules.fail_since` store it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FailStreak {
  /// Consecutive failing runs.
  pub count: i32,
  /// Unix seconds when the count last left 0.
  pub since: Option<i64>,
}

/// What a run does to the streak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreakWrite {
  /// Nothing to write.
  Keep,
  /// Back to 0, written only because it was above 0.
  Reset,
  /// The new streak.
  Set(FailStreak),
  /// The new streak, and the rule is disabled with `paused_reason`.
  Pause {
    /// The streak that paused it.
    streak: FailStreak,
    /// `auto:<status>`.
    reason: String,
  },
}

/// The streak after a run of `status` at `now`.
pub fn next_streak(current: FailStreak, status: RunStatus, now: i64) -> StreakWrite {
  if status == RunStatus::Ok {
    return if current.count > 0 {
      StreakWrite::Reset
    } else {
      StreakWrite::Keep
    };
  }
  if !status.counts_toward_pause() {
    return StreakWrite::Keep;
  }
  let since = match current {
    FailStreak {
      count: 1..,
      since: Some(since),
    } => since,
    _ => now,
  };
  let streak = FailStreak {
    count: current.count.saturating_add(1),
    since: Some(since),
  };
  let old_enough = status != RunStatus::DispatchFailed || now - since >= DISPATCH_FAILED_PAUSE_SECS;
  if streak.count < AUTO_PAUSE_RUNS || !old_enough {
    return StreakWrite::Set(streak);
  }
  let name = status.as_str();
  let mut reason = String::with_capacity(5 + name.len());
  reason.push_str("auto:");
  reason.push_str(name);
  StreakWrite::Pause { streak, reason }
}

/// Whether a failed run leaves a `rule_runs` row. A run that did nothing wrong but was skipped
/// leaves one only while watched; a version mismatch only once it has outlived a deploy.
pub fn failure_keeps_row(status: RunStatus, watched: bool, mismatch_age_secs: i64) -> bool {
  match status {
    RunStatus::Ok => false,
    RunStatus::Skipped => watched,
    RunStatus::VersionMismatch => mismatch_age_secs >= MISMATCH_ROW_AFTER_SECS,
    _ => true,
  }
}

/// One row of `rule_alert_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertFlag {
  /// Whether the condition holds.
  pub firing: bool,
  /// The last reading of the batch that last changed it, unix seconds.
  pub decided_at: i64,
  /// When it last changed, unix seconds of dovecote's clock.
  pub changed_at: i64,
  /// When a mail was last claimed for it, sent or suppressed by a cap.
  pub last_sent_at: Option<i64>,
}

/// A rule's flags for one pigeon and its flock, by scope and key.
pub type AlertFlags = HashMap<(RuleAlertScope, String), AlertFlag>;

/// Whether a `fire` or `clear` moves a flag to `firing`: it must change, the batch must be newer
/// than the batch that last changed it, and a flock flag must have held for
/// [`FLOCK_FLAG_HOLD_SECS`]. A `clear` with no flag does nothing.
///
/// The plan applies this to its snapshot and the transition statement re-applies it to the row
/// it finds, which is what makes two racing batches mail once.
///
/// An older batch never undoes a newer change. Restating a flag, including a `clear` with no
/// flag, writes nothing, so a race can lose a restatement: an older batch that changes the flag
/// lands over a newer one that only restated it, and the next batch corrects it with one more
/// mail. That needs two batches of one key in flight at once; a flock flag's hold bounds it.
pub fn transition_applies(
  current: Option<&AlertFlag>,
  firing: bool,
  scope: RuleAlertScope,
  last_reading: i64,
  now: i64,
) -> bool {
  match current {
    None => firing,
    Some(flag) => {
      flag.firing != firing
        && last_reading > flag.decided_at
        && (scope == RuleAlertScope::Pigeon || now - flag.changed_at >= FLOCK_FLAG_HOLD_SECS)
    }
  }
}

/// Whether a `notify` mails: once per key within [`NOTIFY_REPEAT_SECS`].
pub fn notify_applies(current: Option<&AlertFlag>, now: i64) -> bool {
  current
    .and_then(|flag| flag.last_sent_at)
    .is_none_or(|sent| now - sent >= NOTIFY_REPEAT_SECS)
}

/// A rule's own mail counters as `rules` stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuleMailCounters {
  /// Unix seconds of the hour `in_hour` counts.
  pub hour_start: Option<i64>,
  /// Mails claimed in that hour.
  pub in_hour: i32,
  /// The UTC day `in_day` counts, as days since the epoch.
  pub day: Option<i64>,
  /// Mails claimed that day.
  pub in_day: i32,
}

/// Mail already claimed in the current hour and day, per rule, account and platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MailCounts {
  /// This rule, this hour.
  pub rule_hour: i32,
  /// This rule, today.
  pub rule_day: i32,
  /// This rule's account, today.
  pub account_day: i32,
  /// Every account, today.
  pub platform_day: i32,
}

impl MailCounts {
  /// The counts at `now`: the rule's counters roll to zero at a new hour or day, while the
  /// account and platform counts are read from today's `rule_usage` rows. Build one per rule, and
  /// carry its account and platform counts into the next rule of the dispatch, so the plan claims
  /// what the claim statement allows.
  pub fn at(rule: RuleMailCounters, account_day: i32, platform_day: i32, now: i64) -> Self {
    let hour = now - now.rem_euclid(3_600);
    let day = now.div_euclid(86_400);
    Self {
      rule_hour: if rule.hour_start == Some(hour) {
        rule.in_hour
      } else {
        0
      },
      rule_day: if rule.day == Some(day) {
        rule.in_day
      } else {
        0
      },
      account_day,
      platform_day,
    }
  }

  /// Claims one mail under every cap, or none past any of them.
  pub fn claim(&mut self) -> bool {
    let room = self.rule_hour < RULE_MAIL_PER_HOUR
      && self.rule_day < RULE_MAIL_PER_DAY
      && self.account_day < ACCOUNT_RULE_MAIL_PER_DAY
      && self.platform_day < PLATFORM_RULE_MAIL_PER_DAY;
    if room {
      self.rule_hour += 1;
      self.rule_day += 1;
      self.account_day += 1;
      self.platform_day += 1;
    }
    room
  }
}

/// A flag that moves, and its mail.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertDecision {
  /// The alert as the rule gave it: words, scope and key.
  pub alert: RuleAlert,
  /// The row to write.
  pub flag: AlertFlag,
  /// False when a cap suppressed the mail; the flag moves all the same.
  pub mailed: bool,
}

/// The mail decision, one function for the live path and the dry run, so "would have mailed"
/// means what live mails. A restated, unchanged flag decides nothing (see
/// [`transition_applies`]).
pub fn decide_alerts(
  flags: &AlertFlags,
  alerts: &[RuleAlert],
  last_reading: i64,
  now: i64,
  counts: &mut MailCounts,
) -> Vec<AlertDecision> {
  let mut decisions = Vec::new();
  for alert in alerts {
    let current = flags.get(&(alert.scope, alert.key.clone()));
    let flag = match alert.kind {
      RuleAlertKind::Notify => {
        if !notify_applies(current, now) {
          continue;
        }
        AlertFlag {
          firing: current.is_some_and(|f| f.firing),
          decided_at: current.map_or(last_reading, |f| f.decided_at),
          changed_at: current.map_or(now, |f| f.changed_at),
          last_sent_at: Some(now),
        }
      }
      RuleAlertKind::Fire | RuleAlertKind::Clear => {
        let firing = alert.kind == RuleAlertKind::Fire;
        if !transition_applies(current, firing, alert.scope, last_reading, now) {
          continue;
        }
        AlertFlag {
          firing,
          decided_at: last_reading,
          changed_at: now,
          last_sent_at: Some(now),
        }
      }
    };
    decisions.push(AlertDecision {
      alert: alert.clone(),
      flag,
      mailed: counts.claim(),
    });
  }
  decisions
}

/// A derived value to insert, `ON CONFLICT DO NOTHING` on its key and time.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedValue {
  /// The value's key.
  pub key: String,
  /// The value as the rule gave it.
  pub value: String,
  /// Parsed as history parses a telemetry value.
  pub value_num: Option<f64>,
  /// Clamped into the batch.
  pub at: i64,
}

/// Everything one good result writes, in one transaction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ApplyPlan {
  /// The patch for the pigeon document, only when it changes it.
  pub pigeon_patch: Option<Map<String, Value>>,
  /// The patch for the flock document, only when it changes it.
  pub flock_patch: Option<Map<String, Value>>,
  /// Values to insert.
  pub values: Vec<PlannedValue>,
  /// Flags that move, with their mails.
  pub alerts: Vec<AlertDecision>,
  /// Whether a `rule_runs` row is kept. It is inserted `ON CONFLICT DO NOTHING` on `(rule_id,
  /// pigeon_id, batch_id)`, so a redelivered batch keeps the first delivery's row.
  pub run_row: bool,
}

impl ApplyPlan {
  /// Statements the plan runs: one per patch, one insert for all values, one per flag and the
  /// run row. A quiet run is zero. The streak write is [`next_streak`]'s, not the plan's.
  pub fn statements(&self) -> usize {
    usize::from(self.pigeon_patch.is_some())
      + usize::from(self.flock_patch.is_some())
      + usize::from(!self.values.is_empty())
      + self.alerts.len()
      + usize::from(self.run_row)
  }
}

/// What the fresh read knew about one rule when its batch was dispatched.
#[derive(Debug, Clone, Copy)]
pub struct RunContext<'a> {
  /// The documents the rule was given.
  pub state: &'a RuleState,
  /// Its flags for this pigeon and the flock.
  pub flags: &'a AlertFlags,
  /// The batch's first reading, unix seconds.
  pub first_reading: i64,
  /// The batch's last reading, unix seconds.
  pub last_reading: i64,
  /// dovecote's clock.
  pub now: i64,
  /// Inside the watch window: every run keeps a row.
  pub watched: bool,
}

/// A patch would grow one dispatch's state past [`MAX_RULE_STATE_TOTAL_BYTES`]: that rule's
/// `bad_output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateBudgetExceeded;

impl StateBudgetExceeded {
  /// The run's `error`.
  pub fn as_str(self) -> &'static str {
    "flock state budget"
  }
}

/// Plans one validated result. `state_total` is the dispatch's running state size from
/// [`state_total`], updated when the plan is accepted; a patch that would grow it past the budget
/// refuses the whole result before any mail is claimed from `counts`.
pub fn plan_apply(
  result: &RuleResult,
  logs: &[String],
  ctx: &RunContext,
  state_total: &mut usize,
  counts: &mut MailCounts,
) -> Result<ApplyPlan, StateBudgetExceeded> {
  let pigeon = changed(&ctx.state.pigeon, result.state.pigeon.as_ref());
  let flock = changed(&ctx.state.flock, result.state.flock.as_ref());
  let before = state_doc_bytes(&ctx.state.pigeon).saturating_add(state_doc_bytes(&ctx.state.flock));
  let after = pigeon
    .as_ref()
    .map_or(state_doc_bytes(&ctx.state.pigeon), |(_, merged)| {
      state_doc_bytes(merged)
    })
    .saturating_add(
      flock
        .as_ref()
        .map_or(state_doc_bytes(&ctx.state.flock), |(_, merged)| {
          state_doc_bytes(merged)
        }),
    );
  let total = state_total.saturating_sub(before).saturating_add(after);
  if total > MAX_RULE_STATE_TOTAL_BYTES && total > *state_total {
    return Err(StateBudgetExceeded);
  }
  *state_total = total;

  let mut values: Vec<PlannedValue> = Vec::with_capacity(result.values.len());
  for v in &result.values {
    let at = v
      .at
      .unwrap_or(ctx.last_reading)
      .max(ctx.first_reading)
      .min(ctx.last_reading);
    if values.iter().any(|p| p.key == v.key && p.at == at) {
      continue;
    }
    values.push(PlannedValue {
      key: v.key.clone(),
      value: v.value.clone(),
      value_num: v.value.parse::<f64>().ok(),
      at,
    });
  }
  let alerts = decide_alerts(ctx.flags, &result.alerts, ctx.last_reading, ctx.now, counts);
  let mut plan = ApplyPlan {
    pigeon_patch: pigeon.map(|(patch, _)| patch),
    flock_patch: flock.map(|(patch, _)| patch),
    values,
    alerts,
    run_row: false,
  };
  plan.run_row = ctx.watched
    || !logs.is_empty()
    || !plan.values.is_empty()
    || !plan.alerts.is_empty()
    || plan.pigeon_patch.is_some()
    || plan.flock_patch.is_some();
  Ok(plan)
}

/// The patch and the merged document, when the patch changes the document.
fn changed(
  stored: &Map<String, Value>,
  patch: Option<&Map<String, Value>>,
) -> Option<(Map<String, Value>, Map<String, Value>)> {
  let patch = patch?;
  let merged = merge_state(stored, patch);
  (merged != *stored).then(|| (patch.clone(), merged))
}

/// One enabled rule as the second pass's fresh read found it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRule {
  /// The rule's id.
  pub id: Uuid,
  /// Its deployed version.
  pub version: i32,
  /// Its documents for this pigeon and the flock.
  pub state: RuleState,
  /// Its flags for this pigeon and the flock.
  pub flags: AlertFlags,
  /// Its own mail counters.
  pub mail: RuleMailCounters,
  /// Its failure streak is under way, so an `ok` run has a reset to write.
  pub failing: bool,
  /// Inside its watch window: every run keeps a row.
  pub watched: bool,
  /// A version of it is pending: a deploy is in flight, so a mismatch is expected.
  pub pending: bool,
}

/// What the second pass's fresh read found for one batch's pigeon.
#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
  /// Postgres's clock at the read, unix seconds: the run's `now` and the event's `received_at`.
  pub now: i64,
  /// The pigeon's flock.
  pub flock: RuleNamedRef,
  /// The pigeon.
  pub pigeon: RuleNamedRef,
  /// The flock's owning organization, else its user: whose mail it is and whose failure the
  /// breaker counts.
  pub account: Uuid,
  /// The flock's organization; `None` for a personal flock, which `RULES_ALLOWED_ORG_IDS` can
  /// never list.
  pub org: Option<Uuid>,
  /// When the flock's script was last uploaded. `None` means it never was, so a dispatch could
  /// only find no script, which is the platform's doing, never the flock's.
  pub uploaded_at: Option<i64>,
  /// The flock's deploy lease is held.
  pub leased: bool,
  /// Rule mail claimed today under the account.
  pub account_mail_day: i32,
  /// Rule mail claimed today across the platform.
  pub platform_mail_day: i32,
  /// Every enabled, unblocked, deployed rule of the flock, in the order dispatched.
  pub rules: Vec<LiveRule>,
}

impl Lookup {
  /// The rules as the request names them.
  pub fn dispatch_rules(&self) -> Vec<DispatchRule> {
    self
      .rules
      .iter()
      .map(|rule| DispatchRule {
        id: uuid_str(rule.id),
        version: rule.version,
        state: rule.state.clone(),
      })
      .collect()
  }
}

/// What one rule's run writes, decided before any statement.
#[derive(Debug, Clone, PartialEq)]
pub struct RulePlan {
  /// The rule.
  pub rule_id: Uuid,
  /// The version dispatched.
  pub version: i32,
  /// The run's outcome.
  pub status: RunStatus,
  /// User code's own text or a refusal.
  pub error: Option<String>,
  /// Captured `console.log` lines.
  pub logs: Vec<String>,
  /// The writes of an `ok` result.
  pub apply: Option<ApplyPlan>,
  /// Whether a `rule_runs` row is kept.
  pub run_row: bool,
  /// Whether the run writes anything, its streak included: a healthy rule's quiet `ok` and a
  /// deploy's expected mismatch do not.
  pub writes: bool,
}

/// The error of a run whose result passed every check but Postgres refused to store.
const UNSTORABLE: &str = "result could not be stored";

impl RulePlan {
  /// This run as `bad_output` with nothing applied, for when Postgres refused its writes: a
  /// refusal that a check missed must still count toward the pause rather than recur unseen.
  /// Logs are dropped, since they may be what was refused.
  pub fn unstorable(&self) -> RulePlan {
    RulePlan {
      rule_id: self.rule_id,
      version: self.version,
      status: RunStatus::BadOutput,
      error: Some(UNSTORABLE.to_string()),
      logs: Vec::new(),
      apply: None,
      run_row: true,
      writes: true,
    }
  }
}

/// Plans each rule's record of one dispatch by position: the i-th rule sent owns the i-th result,
/// which [`classify`] has matched by id. A platform failure records nothing, nor does a whole
/// failure while our own deploy holds the flock's lease, since the script may then be mid-upload
/// or already deleted. Mail counts carry from rule to rule, so the plan claims what the claim
/// statement allows.
pub fn plan_record(
  lookup: &Lookup,
  readings: &[ResolvedReading],
  classified: &Classified,
) -> Vec<RulePlan> {
  let records = match classified {
    Classified::Platform => return Vec::new(),
    Classified::Flock(_) if lookup.leased => return Vec::new(),
    Classified::PerRule(records) => Some(records),
    Classified::Flock(_) => None,
  };
  let now = lookup.now;
  let first = readings.iter().map(|r| r.at_secs).min().unwrap_or(now);
  let last = readings.iter().map(|r| r.at_secs).max().unwrap_or(now);
  let mut total = state_total(&lookup.dispatch_rules());
  let (mut account_day, mut platform_day) = (lookup.account_mail_day, lookup.platform_mail_day);
  let mut plans = Vec::with_capacity(lookup.rules.len());
  for (i, rule) in lookup.rules.iter().enumerate() {
    let record = records.and_then(|records| records.get(i));
    let mut plan = RulePlan {
      rule_id: rule.id,
      version: rule.version,
      status: match (classified, record) {
        (Classified::Flock(status), _) => *status,
        (_, Some(record)) => record.status,
        _ => RunStatus::DispatchFailed,
      },
      error: record.and_then(|r| r.error.clone()),
      logs: record.map(|r| r.logs.clone()).unwrap_or_default(),
      apply: None,
      run_row: false,
      writes: false,
    };
    if let Some(result) = record.and_then(|r| r.result.as_ref()) {
      let ctx = RunContext {
        state: &rule.state,
        flags: &rule.flags,
        first_reading: first,
        last_reading: last,
        now,
        watched: rule.watched,
      };
      let mut counts = MailCounts::at(rule.mail, account_day, platform_day, now);
      match plan_apply(result, &plan.logs, &ctx, &mut total, &mut counts) {
        Ok(apply) => {
          account_day = counts.account_day;
          platform_day = counts.platform_day;
          plan.run_row = apply.run_row;
          plan.writes = apply.statements() > 0 || rule.failing;
          plan.apply = Some(apply);
          plans.push(plan);
          continue;
        }
        Err(refused) => {
          plan.status = RunStatus::BadOutput;
          plan.error = Some(refused.as_str().to_string());
        }
      }
    }
    let mismatch_age = match lookup.uploaded_at {
      Some(at) if !rule.pending && !lookup.leased => now - at,
      _ => 0,
    };
    plan.run_row = failure_keeps_row(plan.status, rule.watched, mismatch_age);
    plan.writes = plan.run_row || plan.status.counts_toward_pause();
    plans.push(plan);
  }
  plans
}

/// The send-time recipient recheck alert mail and rule mail share: the definition's own list, each
/// address kept only while it is still the flock owner's or an organization member's, and the
/// owner when none is left. `members` is a comma-separated list. Returns the addresses and how
/// many were dropped.
pub fn deliverable_recipients(
  to: &[String],
  owner: Option<&str>,
  members: Option<&str>,
) -> (Vec<String>, usize) {
  let owner_only: Vec<String> = owner.iter().map(|o| (*o).to_string()).collect();
  let known = |address: &str| {
    owner
      .into_iter()
      .chain(members.into_iter().flat_map(|list| list.split(',')))
      .any(|known| known.trim().eq_ignore_ascii_case(address))
  };
  let kept: Vec<String> = to
    .iter()
    .map(|address| address.trim())
    .filter(|address| known(address))
    .map(str::to_string)
    .collect();
  let refused = to.len() - kept.len();
  if kept.is_empty() {
    (owner_only, refused)
  } else {
    (kept, refused)
  }
}

/// The subject prefix every rule mail carries, so a rule's words never pass for the platform's.
const RULE_MAIL_PREFIX: &str = "PidgeIoT rule: ";

/// A rule mail's subject and text: the rule's own words under a fixed prefix, then a footer naming
/// the severity, rule, flock and pigeon, so a recipient can tell who wrote it and where to stop it.
pub fn rule_mail_message(
  alert: &RuleAlert,
  rule: &str,
  flock: &str,
  pigeon: &str,
) -> (String, String) {
  let mut subject = String::with_capacity(RULE_MAIL_PREFIX.len() + alert.subject.len());
  subject.push_str(RULE_MAIL_PREFIX);
  subject.push_str(&alert.subject);
  let severity = match alert.severity {
    RuleSeverity::Info => "info",
    RuleSeverity::Warning => "warning",
    RuleSeverity::Critical => "critical",
  };
  let body = alert.body.as_deref().unwrap_or("");
  const GAP: &str = "\n\n";
  const SEVERITY: &str = "--\nSeverity: ";
  const RULE: &str = "\nRule: ";
  const FLOCK: &str = "\nFlock: ";
  const PIGEON: &str = "\nPigeon: ";
  const STOP: &str = "\n\nA flock owner or admin can change this rule's recipients or disable it \
                      on the flock's Rules page.\n";
  let mut text = String::with_capacity(
    body.len()
      + GAP.len()
      + SEVERITY.len()
      + severity.len()
      + RULE.len()
      + rule.len()
      + FLOCK.len()
      + flock.len()
      + PIGEON.len()
      + pigeon.len()
      + STOP.len(),
  );
  if !body.is_empty() {
    text.push_str(body);
    text.push_str(GAP);
  }
  text.push_str(SEVERITY);
  text.push_str(severity);
  text.push_str(RULE);
  text.push_str(rule);
  text.push_str(FLOCK);
  text.push_str(flock);
  text.push_str(PIGEON);
  text.push_str(pigeon);
  text.push_str(STOP);
  (subject, text)
}

const PAUSED_PREFIX: &str = "PidgeIoT rule paused: ";

/// The mail to a rule's author when failing runs paused it: the status that paused it and the
/// pausing run's error, which is the rule's own text, then how to start it again.
pub fn rule_paused_message(
  rule: &str,
  flock: &str,
  reason: &str,
  error: Option<&str>,
) -> (String, String) {
  let mut subject = String::with_capacity(PAUSED_PREFIX.len() + rule.len());
  subject.push_str(PAUSED_PREFIX);
  subject.push_str(rule);
  let status = reason.strip_prefix("auto:").unwrap_or(reason);
  let runs = AUTO_PAUSE_RUNS.to_string();
  const HEAD: &str = "This rule failed ";
  const RUNS: &str = " runs in a row, the last as ";
  const PAUSED: &str = ", and is paused: it runs on no batch until it is enabled again.";
  const ERROR: &str = "\n\nIts last error:\n";
  const RULE: &str = "\n\n--\nRule: ";
  const FLOCK: &str = "\nFlock: ";
  const STOP: &str = "\n\nA flock owner or admin can test and deploy a fixed version, or enable \
                      the rule again, on the flock's Rules page.\n";
  let error = error.unwrap_or("");
  let mut text = String::with_capacity(
    HEAD.len()
      + runs.len()
      + RUNS.len()
      + status.len()
      + PAUSED.len()
      + ERROR.len()
      + error.len()
      + RULE.len()
      + rule.len()
      + FLOCK.len()
      + flock.len()
      + STOP.len(),
  );
  text.push_str(HEAD);
  text.push_str(&runs);
  text.push_str(RUNS);
  text.push_str(status);
  text.push_str(PAUSED);
  if !error.is_empty() {
    text.push_str(ERROR);
    text.push_str(error);
  }
  text.push_str(RULE);
  text.push_str(rule);
  text.push_str(FLOCK);
  text.push_str(flock);
  text.push_str(STOP);
  (subject, text)
}

/// Which script a name is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
  /// A flock's live bundle, `flock-<flock uuid>`.
  Flock,
  /// A rule's dry run, `test-<uuid>` under a name of its own ([`test_script_name`]).
  Test,
  /// The same script uploaded again for the dry run's CPU check, at the live limit.
  CpuCheck,
}

/// The environment a script is tagged with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesEnv {
  /// `env-prod`.
  Production,
  /// `env-staging`.
  Staging,
}

pub(crate) fn is_uuid(s: &str) -> bool {
  s.len() == 36
    && s.bytes().enumerate().all(|(i, b)| match i {
      8 | 13 | 18 | 23 => b == b'-',
      _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
    })
}

/// The script's name from a canonical lowercase uuid, `None` for anything else, so no request
/// text can name a script.
pub fn script_name(kind: ScriptKind, uuid: &str) -> Option<String> {
  if !is_uuid(uuid) {
    return None;
  }
  let prefix = match kind {
    ScriptKind::Flock => "flock-",
    ScriptKind::Test | ScriptKind::CpuCheck => "test-",
  };
  let mut name = String::with_capacity(prefix.len() + 32);
  name.push_str(prefix);
  name.extend(uuid.chars().filter(|c| *c != '-'));
  Some(name)
}

/// A dry run's script name, `test-` and a fresh v7 uuid. Never a name used before: the namespace
/// can refuse to dispatch a name it has deleted, even after that name is uploaded again.
pub fn test_script_name() -> Option<String> {
  script_name(ScriptKind::Test, &uuid_str(Uuid::now_v7()))
}

/// One rule module of a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleRule<'a> {
  /// Its key in `rules.js` and its file `r-<key>.js`: the rule's uuid in a flock bundle, or
  /// `<uuid>-v<version>` where a dry run holds two versions of one rule.
  pub key: &'a str,
  /// The version the harness checks a dispatch against.
  pub version: i32,
  /// The module source.
  pub source: &'a str,
}

fn is_bundle_key(key: &str) -> bool {
  match (key.get(..36), key.get(36..)) {
    (Some(uuid), Some("")) => is_uuid(uuid),
    (Some(uuid), Some(rest)) => {
      is_uuid(uuid)
        && rest
          .strip_prefix("-v")
          .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }
    _ => false,
  }
}

fn bundle_ok(rules: &[BundleRule]) -> bool {
  rules.iter().enumerate().all(|(i, rule)| {
    is_bundle_key(rule.key) && rule.version >= 1 && rules[..i].iter().all(|r| r.key != rule.key)
  })
}

const MANIFEST_HEAD: &str =
  "// Generated by dovecote: each rule this script holds, at its version.\n";
const MANIFEST_EXPORT: &str = "\nexport default {\n";
const MANIFEST_TAIL: &str = "};\n";

/// The bundle's generated `rules.js`, or `None` for a key that is not a uuid, a version below 1,
/// or a repeated key. Keys and versions are the only text it writes into JavaScript.
pub fn manifest(rules: &[BundleRule]) -> Option<String> {
  if !bundle_ok(rules) {
    return None;
  }
  let numbers: Vec<(String, String)> = rules
    .iter()
    .enumerate()
    .map(|(i, rule)| (i.to_string(), rule.version.to_string()))
    .collect();
  let entries: usize = rules
    .iter()
    .zip(&numbers)
    .map(|(rule, (i, version))| {
      "import r from \"./r-.js\";\n".len()
        + "  \"\": { version: , run: r },\n".len()
        + 2 * rule.key.len()
        + 2 * i.len()
        + version.len()
    })
    .sum();
  let mut src = String::with_capacity(
    MANIFEST_HEAD.len() + MANIFEST_EXPORT.len() + MANIFEST_TAIL.len() + entries,
  );
  src.push_str(MANIFEST_HEAD);
  for (rule, (i, _)) in rules.iter().zip(&numbers) {
    src.push_str("import r");
    src.push_str(i);
    src.push_str(" from \"./r-");
    src.push_str(rule.key);
    src.push_str(".js\";\n");
  }
  src.push_str(MANIFEST_EXPORT);
  for (rule, (i, version)) in rules.iter().zip(&numbers) {
    src.push_str("  \"");
    src.push_str(rule.key);
    src.push_str("\": { version: ");
    src.push_str(version);
    src.push_str(", run: r");
    src.push_str(i);
    src.push_str(" },\n");
  }
  src.push_str(MANIFEST_TAIL);
  Some(src)
}

/// Every module of a bundle as (file name, source): the harness as compiled in, the generated
/// `rules.js`, then one `r-<key>.js` per rule.
pub fn bundle_modules(rules: &[BundleRule]) -> Option<Vec<(String, String)>> {
  let manifest = manifest(rules)?;
  let mut modules = Vec::with_capacity(HARNESS.len() + 1 + rules.len());
  for (name, source) in HARNESS {
    modules.push((name.to_string(), source.to_string()));
  }
  modules.push(("rules.js".to_string(), manifest));
  for rule in rules {
    let mut name = String::with_capacity(rule.key.len() + 5);
    name.push_str("r-");
    name.push_str(rule.key);
    name.push_str(".js");
    modules.push((name, rule.source.to_string()));
  }
  Some(modules)
}

/// `a` then `b`, in one allocation of their size.
fn concat2(a: &str, b: &str) -> String {
  let mut s = String::with_capacity(a.len() + b.len());
  s.push_str(a);
  s.push_str(b);
  s
}

/// The tag every rules script carries, which the sweep lists by.
pub const RULE_SCRIPT_TAG: &str = "pidgeiot-rule";

/// A uuid as script names and tags spell it: hyphenated and lowercase.
pub(crate) fn uuid_str(id: Uuid) -> String {
  let mut buf = Uuid::encode_buffer();
  id.hyphenated().encode_lower(&mut buf).to_string()
}

/// The `acct-` tag of an owning organization or user, spelled only here: [`script_tags`] writes
/// it and `rule_bundles.account_tag` records it, and the sweep re-tags wherever the two differ.
pub fn account_tag(account: &str) -> String {
  concat2("acct-", account)
}

/// A script's tags: [`RULE_SCRIPT_TAG`], `acct-<account>`, `flock-<subject>` (a dry run's `test`)
/// and the environment's. An upload and a re-tag both state all four, so neither leaves one
/// behind. `None` unless `account` and `subject` are canonical uuids.
pub fn script_tags(
  kind: ScriptKind,
  account: &str,
  subject: &str,
  env: RulesEnv,
) -> Option<[String; 4]> {
  if !is_uuid(account) || !is_uuid(subject) {
    return None;
  }
  let subject_tag = match kind {
    ScriptKind::Flock => concat2("flock-", subject),
    ScriptKind::Test | ScriptKind::CpuCheck => "test".to_string(),
  };
  let env_tag = match env {
    RulesEnv::Production => "env-prod",
    RulesEnv::Staging => "env-staging",
  };
  Some([
    RULE_SCRIPT_TAG.to_string(),
    account_tag(account),
    subject_tag,
    env_tag.to_string(),
  ])
}

/// The upload's metadata part, the only place the CPU limit binds. Every field is stated so
/// nothing is inherited: no bindings, CPU per [`ScriptKind`], observability off. `subrequests: 0`
/// stops no egress; the namespace's outbound Worker does. `subject` is the flock's uuid for a
/// live bundle and the rule's for a dry run; `account` is the owning org or user. A rule supplies
/// none of it, which is what keeps it from naming a binding: the API accepts one bound to any
/// class on the account, dovecote's `Pigeons` included.
pub fn upload_metadata(
  kind: ScriptKind,
  account: &str,
  subject: &str,
  env: RulesEnv,
) -> Option<String> {
  let tags = script_tags(kind, account, subject, env)?;
  let cpu_ms = match kind {
    ScriptKind::Flock | ScriptKind::CpuCheck => RULE_CPU_MS,
    ScriptKind::Test => TEST_RULE_CPU_MS,
  };
  let metadata = serde_json::json!({
    "main_module": "main.js",
    "compatibility_date": RULES_COMPATIBILITY_DATE,
    "bindings": [],
    "limits": { "cpu_ms": cpu_ms, "subrequests": 0 },
    "tags": tags,
    "observability": { "enabled": false },
  });
  Some(metadata.to_string())
}

/// The upload's `Content-Type` header.
pub fn multipart_content_type(boundary: &str) -> String {
  concat2("multipart/form-data; boundary=", boundary)
}

const PART_METADATA: &str =
  "Content-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n";
const PART_MODULE_TYPE: &str = "\r\nContent-Type: application/javascript+module\r\n\r\n";

/// The upload body, built by hand because `worker::FormData` cannot give a part a content type.
///
/// `boundary` is 32 random lowercase hex from the caller; `None` when it is not, when it occurs in
/// any part, or when a file name could break its header.
pub fn multipart_body(
  boundary: &str,
  metadata: &str,
  modules: &[(String, String)],
) -> Option<String> {
  if boundary.len() != 32 || !is_lower_hex(boundary) {
    return None;
  }
  let name_ok = |n: &str| {
    !n.is_empty()
      && n
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
  };
  if metadata.contains(boundary)
    || modules
      .iter()
      .any(|(name, source)| !name_ok(name) || source.contains(boundary))
  {
    return None;
  }
  // Each part opens with "--boundary\r\n" and closes with "\r\n"; the body ends "--boundary--\r\n".
  let open = 2 + boundary.len() + 2;
  let module_head = "Content-Disposition: form-data; name=\"\"; filename=\"\"".len();
  let len = open
    + PART_METADATA.len()
    + metadata.len()
    + 2
    + modules
      .iter()
      .map(|(name, source)| {
        open + module_head + 2 * name.len() + PART_MODULE_TYPE.len() + source.len() + 2
      })
      .sum::<usize>()
    + 2
    + boundary.len()
    + 4;
  let mut body = String::with_capacity(len);
  let open_part = |body: &mut String| {
    body.push_str("--");
    body.push_str(boundary);
    body.push_str("\r\n");
  };
  open_part(&mut body);
  body.push_str(PART_METADATA);
  body.push_str(metadata);
  body.push_str("\r\n");
  for (name, source) in modules {
    open_part(&mut body);
    body.push_str("Content-Disposition: form-data; name=\"");
    body.push_str(name);
    body.push_str("\"; filename=\"");
    body.push_str(name);
    body.push('"');
    body.push_str(PART_MODULE_TYPE);
    body.push_str(source);
    body.push_str("\r\n");
  }
  body.push_str("--");
  body.push_str(boundary);
  body.push_str("--\r\n");
  Some(body)
}

/// Regroups a flock's history rows, oldest first as the history read returns them, into the
/// events a dry run replays: one reading per pigeon and timestamp, `previous` rebuilt per key
/// from the pigeon's whole history, the newest [`REPLAY_MAX_READINGS`] readings kept, in batches
/// of up to [`MAX_TELEMETRY_BATCH_READINGS`] per pigeon, ordered by each batch's last reading.
///
/// Field pigeons are left out by the history read's SQL, never filtered here, so their rows are
/// never read at all.
pub fn regroup_history(
  points: &[TelemetryHistoryPoint],
  flock: &RuleNamedRef,
  names: &HashMap<String, String>,
) -> Vec<CommonEvent> {
  let mut per_pigeon: BTreeMap<&str, BTreeMap<i64, BTreeMap<String, String>>> = BTreeMap::new();
  for p in points {
    per_pigeon
      .entry(p.pigeon_id.as_str())
      .or_default()
      .entry(p.reported_at.unix_timestamp())
      .or_default()
      .insert(p.key.clone(), p.value.clone());
  }
  // `previous` is rebuilt before the cut, so the oldest kept reading still sees what it displaced.
  let mut all: Vec<(&str, RuleReading)> = Vec::new();
  for (pigeon, readings) in per_pigeon {
    let mut last: BTreeMap<String, RulePrevious> = BTreeMap::new();
    for (at, metrics) in readings {
      let reading = with_previous(&mut last, at, metrics);
      all.push((pigeon, reading));
    }
  }
  all.sort_by_key(|(_, r)| r.at);
  let kept = all.split_off(all.len().saturating_sub(REPLAY_MAX_READINGS));

  let mut per_pigeon: BTreeMap<&str, Vec<RuleReading>> = BTreeMap::new();
  for (pigeon, reading) in kept {
    per_pigeon.entry(pigeon).or_default().push(reading);
  }
  let mut events = Vec::new();
  for (pigeon, readings) in per_pigeon {
    let name = names.get(pigeon).cloned().unwrap_or_default();
    for chunk in readings.chunks(MAX_TELEMETRY_BATCH_READINGS) {
      events.push(synthetic_event(flock, pigeon, &name, chunk.to_vec()));
    }
  }
  events.sort_by_key(|e| e.received_at);
  events
}

/// A reading whose `previous` is what `last` held for its keys; `last` then takes its values.
fn with_previous(
  last: &mut BTreeMap<String, RulePrevious>,
  at: i64,
  metrics: BTreeMap<String, String>,
) -> RuleReading {
  let previous = metrics
    .keys()
    .filter_map(|k| last.get(k).map(|p| (k.clone(), p.clone())))
    .collect();
  for (k, v) in &metrics {
    let entry = RulePrevious {
      value: v.clone(),
      at,
    };
    last.insert(k.clone(), entry);
  }
  RuleReading {
    at,
    metrics,
    previous,
  }
}

/// An event that never came off the queue: its `batch_id` hashes the readings it carries.
fn synthetic_event(
  flock: &RuleNamedRef,
  pigeon: &str,
  name: &str,
  readings: Vec<RuleReading>,
) -> CommonEvent {
  let payload = serde_json::to_string(&readings).unwrap_or_default();
  CommonEvent {
    contract: RULE_CONTRACT,
    hook: RuleHook::Telemetry,
    batch_id: batch_id(pigeon, &payload),
    received_at: readings.last().map_or(0, |r| r.at),
    degraded: false,
    flock: flock.clone(),
    pigeon: RuleNamedRef {
      id: pigeon.to_string(),
      name: name.to_string(),
    },
    readings,
  }
}

/// Why a test case was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseError {
  /// Not [`MIN_TEST_CASES`] to [`MAX_TEST_CASES`] cases.
  CaseCount,
  /// A case with no events.
  NoEvents,
  /// More than [`MAX_CASE_EVENTS`] events.
  TooManyEvents,
  /// An event with no readings.
  NoReadings,
  /// More than [`MAX_TELEMETRY_BATCH_READINGS`] readings in an event.
  TooManyReadings,
  /// An offset outside 0 to [`MAX_CASE_OFFSET_SECS`].
  Offset,
  /// A key given twice in one reading.
  RepeatedKey,
  /// More than [`MAX_TELEMETRY_KEYS`] keys in one reading.
  TooManyKeys,
  /// An empty key or one over [`MAX_TELEMETRY_KEY_BYTES`].
  BadKey,
  /// A value over [`MAX_TELEMETRY_VALUE_BYTES`].
  BadValue,
}

impl CaseError {
  /// Fixed text for a repair turn and the page.
  pub fn as_str(self) -> &'static str {
    match self {
      CaseError::CaseCount => "two to five test cases",
      CaseError::NoEvents => "a case has no events",
      CaseError::TooManyEvents => "a case has more than eight events",
      CaseError::NoReadings => "an event has no readings",
      CaseError::TooManyReadings => "an event has more than 64 readings",
      CaseError::Offset => "after_secs outside 0 to 7 days",
      CaseError::RepeatedKey => "a key repeated in one reading",
      CaseError::TooManyKeys => "a reading has more than 128 keys",
      CaseError::BadKey => "an empty key or one over 128 bytes",
      CaseError::BadValue => "a value over 1,024 bytes",
    }
  }
}

/// Checks the number of cases a draft carries.
pub fn check_case_count(cases: usize) -> Result<(), CaseError> {
  if (MIN_TEST_CASES..=MAX_TEST_CASES).contains(&cases) {
    Ok(())
  } else {
    Err(CaseError::CaseCount)
  }
}

/// Expands a case into contract events starting at `start`: the metric pairs become a map, each
/// abstract pigeon a synthetic id, each offset a time, `previous` rebuilt per key from the case's
/// earlier readings of that pigeon, `degraded` false.
pub fn expand_case(
  case: &RuleTestCase,
  flock: &RuleNamedRef,
  start: i64,
) -> Result<Vec<CommonEvent>, CaseError> {
  if case.events.is_empty() {
    return Err(CaseError::NoEvents);
  }
  if case.events.len() > MAX_CASE_EVENTS {
    return Err(CaseError::TooManyEvents);
  }
  let mut last: [BTreeMap<String, RulePrevious>; 3] = Default::default();
  let mut events = Vec::with_capacity(case.events.len());
  for event in &case.events {
    if event.readings.is_empty() {
      return Err(CaseError::NoReadings);
    }
    if event.readings.len() > MAX_TELEMETRY_BATCH_READINGS {
      return Err(CaseError::TooManyReadings);
    }
    let mut sorted: Vec<&RuleCaseReading> = event.readings.iter().collect();
    sorted.sort_by_key(|r| r.after_secs);
    let (slot, id, name) = case_pigeon(event.pigeon);
    let mut readings = Vec::with_capacity(sorted.len());
    for reading in sorted {
      if !(0..=MAX_CASE_OFFSET_SECS).contains(&reading.after_secs) {
        return Err(CaseError::Offset);
      }
      let metrics = case_metrics(&reading.metrics)?;
      readings.push(with_previous(
        &mut last[slot],
        start + reading.after_secs,
        metrics,
      ));
    }
    events.push(synthetic_event(flock, &id, name, readings));
  }
  Ok(events)
}

fn case_pigeon(pigeon: RuleCasePigeon) -> (usize, String, &'static str) {
  match pigeon {
    RuleCasePigeon::A => (0, "a".repeat(64), "Pigeon a"),
    RuleCasePigeon::B => (1, "b".repeat(64), "Pigeon b"),
    RuleCasePigeon::C => (2, "c".repeat(64), "Pigeon c"),
  }
}

fn case_metrics(pairs: &[RuleCaseMetric]) -> Result<BTreeMap<String, String>, CaseError> {
  if pairs.len() > MAX_TELEMETRY_KEYS {
    return Err(CaseError::TooManyKeys);
  }
  let mut metrics = BTreeMap::new();
  for pair in pairs {
    if pair.key.is_empty() || pair.key.len() > MAX_TELEMETRY_KEY_BYTES {
      return Err(CaseError::BadKey);
    }
    if pair.value.len() > MAX_TELEMETRY_VALUE_BYTES {
      return Err(CaseError::BadValue);
    }
    if metrics
      .insert(pair.key.clone(), pair.value.clone())
      .is_some()
    {
      return Err(CaseError::RepeatedKey);
    }
  }
  Ok(metrics)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::objects::pigeons::{PreviousTelemetryValue, TelemetryWriteResult};
  use capsules::{RuleAlertKind, RuleAlertScope, RuleEvent, RuleSeverity};
  use serde_json::json;
  use std::collections::HashSet;
  use time::OffsetDateTime;

  const LIVE_WORKED: &str = include_str!("../../rules/canned/live-worked.json");
  const LIVE_HOSTILE: &str = include_str!("../../rules/canned/live-hostile.json");
  const HARNESS_FAILED: &str = include_str!("../../rules/canned/harness-failed.json");
  const BAD_INPUT: &str = include_str!("../../rules/canned/bad-input.json");
  const T0: i64 = 1_793_380_000;

  fn sent(rules: &[(&str, i32)]) -> Vec<DispatchRule> {
    rules
      .iter()
      .map(|(id, version)| DispatchRule {
        id: (*id).to_string(),
        version: *version,
        state: RuleState::default(),
      })
      .collect()
  }

  /// The rules make-canned.mjs dispatched for live-worked.json, with the state it gave each.
  fn worked_sent() -> Vec<DispatchRule> {
    let mut rules = sent(&[("r6", 1), ("r7", 1), ("r10", 1), ("gone", 2)]);
    let down = json!({ "b": { "down": true, "at": T0 - 60 } });
    rules[2].state.flock = down.as_object().cloned().unwrap_or_default();
    rules
  }

  fn hostile_sent() -> Vec<DispatchRule> {
    sent(&[
      ("big", 1),
      ("ok", 1),
      ("lonethrow", 1),
      ("hang", 1),
      ("after", 1),
    ])
  }

  fn answered(body: &str) -> DispatchOutcome {
    DispatchOutcome::Answered {
      status: 200,
      body: body.trim_end().as_bytes().to_vec(),
    }
  }

  fn per_rule(classified: Classified) -> Vec<RunRecord> {
    match classified {
      Classified::PerRule(records) => records,
      other => panic!("expected per-rule records, got {other:?}"),
    }
  }

  fn statuses(records: &[RunRecord]) -> Vec<(&str, RunStatus)> {
    records.iter().map(|r| (r.id.as_str(), r.status)).collect()
  }

  fn reading(at: i64, metrics: &[(&str, &str)]) -> ResolvedReading {
    ResolvedReading::new(
      at,
      metrics
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect(),
    )
  }

  fn named(id: &str, name: &str) -> RuleNamedRef {
    RuleNamedRef {
      id: id.to_string(),
      name: name.to_string(),
    }
  }

  fn ctx<'a>(state: &'a RuleState, flags: &'a AlertFlags, now: i64) -> RunContext<'a> {
    RunContext {
      state,
      flags,
      first_reading: now,
      last_reading: now,
      now,
      watched: false,
    }
  }

  fn alert(key: &str, kind: RuleAlertKind, scope: RuleAlertScope) -> RuleAlert {
    RuleAlert {
      key: key.to_string(),
      kind,
      severity: RuleSeverity::Warning,
      scope,
      subject: "s".to_string(),
      body: None,
    }
  }

  /// What the transition, merge, insert, run row and claim statements do to one rule's rows:
  /// each re-applies its predicate to the row it finds, as the SQL does, rather than trusting the
  /// snapshot the plan was decided on.
  #[derive(Default)]
  struct Store {
    pigeon: Map<String, Value>,
    flock: Map<String, Value>,
    flags: AlertFlags,
    values: HashSet<(String, i64)>,
    runs: HashSet<String>,
    writes: usize,
    mails: usize,
  }

  impl Store {
    fn snapshot(&self) -> RuleState {
      RuleState {
        pigeon: self.pigeon.clone(),
        flock: self.flock.clone(),
      }
    }

    fn apply(&mut self, batch: &str, plan: &ApplyPlan, now: i64) {
      if plan.run_row && self.runs.insert(batch.to_string()) {
        self.writes += 1;
      }
      for (doc, patch) in [
        (&mut self.pigeon, &plan.pigeon_patch),
        (&mut self.flock, &plan.flock_patch),
      ] {
        if let Some(patch) = patch {
          let merged = merge_state(doc, patch);
          if merged != *doc {
            *doc = merged;
            self.writes += 1;
          }
        }
      }
      for v in &plan.values {
        if self.values.insert((v.key.clone(), v.at)) {
          self.writes += 1;
        }
      }
      for d in &plan.alerts {
        let key = (d.alert.scope, d.alert.key.clone());
        let current = self.flags.get(&key);
        let applies = match d.alert.kind {
          RuleAlertKind::Notify => notify_applies(current, now),
          _ => transition_applies(
            current,
            d.flag.firing,
            d.alert.scope,
            d.flag.decided_at,
            now,
          ),
        };
        if applies {
          self.flags.insert(key, d.flag);
          self.writes += 1;
          if d.mailed {
            self.mails += 1;
          }
        }
      }
    }

    /// Plans `result` against this store as it stands and applies the plan, as the batch whose
    /// last reading is `last_reading`.
    fn run(&mut self, result: &RuleResult, last_reading: i64, now: i64) -> ApplyPlan {
      let state = self.snapshot();
      let flags = self.flags.clone();
      let plan = self.plan(result, &state, &flags, last_reading, now);
      self.apply(&last_reading.to_string(), &plan, now);
      plan
    }

    fn plan(
      &self,
      result: &RuleResult,
      state: &RuleState,
      flags: &AlertFlags,
      last_reading: i64,
      now: i64,
    ) -> ApplyPlan {
      let mut context = ctx(state, flags, now);
      context.first_reading = last_reading;
      context.last_reading = last_reading;
      let mut total = state_doc_bytes(&state.pigeon) + state_doc_bytes(&state.flock);
      let mut counts = MailCounts::default();
      plan_apply(result, &[], &context, &mut total, &mut counts).expect("inside the budget")
    }
  }

  #[test]
  fn field_pigeons_are_the_three_signs_by_full_id() {
    let prefixes = ["fa8a8055", "ae3a6d3f", "c8bd5d56"];
    for (id, prefix) in FIELD_PIGEONS.iter().zip(prefixes) {
      assert_eq!(id.len(), 64);
      assert!(
        id.bytes()
          .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
      );
      assert!(id.starts_with(prefix));
    }
  }

  #[test]
  fn each_field_pigeon_is_refused_before_any_statement() {
    let other = "1".repeat(64);
    for var in [
      None,
      Some(""),
      Some("not-an-id,,zz,fa8a8055"),
      Some(other.as_str()),
    ] {
      let ids = excluded_pigeons(var);
      for id in FIELD_PIGEONS {
        assert_eq!(
          pre_gate(id, Some("on"), &ids),
          PreGate::Excluded,
          "{id} with {var:?}"
        );
        let upper = id.to_ascii_uppercase();
        assert_eq!(pre_gate(&upper, Some("on"), &ids), PreGate::Excluded);
        assert_eq!(pre_gate(id, None, &ids), PreGate::Off);
      }
    }
    // Whole ids only: a pigeon sharing a field pigeon's prefix still runs its rules.
    let mut cousin = FIELD_PIGEONS[0].to_string();
    cousin.replace_range(63..64, "0");
    assert_ne!(cousin, FIELD_PIGEONS[0]);
    let field = excluded_pigeons(None);
    assert_eq!(pre_gate(&cousin, Some("on"), &field), PreGate::Lookup);
    assert_eq!(
      pre_gate(&other, Some("on"), &excluded_pigeons(Some(&other))),
      PreGate::Excluded
    );
    assert_eq!(pre_gate(&other, Some("off"), &field), PreGate::Off);
    assert_eq!(pre_gate(&other, Some("on"), &field), PreGate::Lookup);
  }

  #[test]
  fn an_unset_switch_reads_nothing_else() {
    for dispatch in [None, Some(""), Some("off"), Some("ON"), Some("1")] {
      let read = std::cell::RefCell::new(Vec::new());
      let vars = pass_vars(|name| {
        read.borrow_mut().push(name.to_string());
        match name {
          "RULES_DISPATCH" => dispatch.map(str::to_string),
          _ => Some(FIELD_PIGEONS[0].to_string()),
        }
      });
      assert_eq!(vars, None, "{dispatch:?}");
      assert_eq!(read.into_inner(), ["RULES_DISPATCH"], "{dispatch:?}");
    }
    let extra = "3".repeat(64);
    let vars = pass_vars(|name| match name {
      "RULES_DISPATCH" => Some(" on ".to_string()),
      "RULES_EXCLUDED_PIGEONS" => Some(extra.clone()),
      "RULES_ALLOWED_ORG_IDS" => Some("listed".to_string()),
      _ => None,
    });
    let vars = vars.expect("on");
    assert_eq!(vars.allowed_orgs.as_deref(), Some("listed"));
    assert_eq!(vars.excluded, excluded_pigeons(Some(&extra)));
    assert_eq!(vars.excluded.len(), 4);
    assert_eq!(vars.gate(&extra), PreGate::Excluded);
    assert_eq!(vars.gate(FIELD_PIGEONS[2]), PreGate::Excluded);
    assert_eq!(vars.gate(&"4".repeat(64)), PreGate::Lookup);
  }

  #[test]
  fn the_pass_budget_alone() {
    let ledger = PassLedger::new(5_000);
    assert!(!ledger.budget_spent(5_000));
    assert!(!ledger.budget_spent(5_000 + PASS_BUDGET_MS - 1));
    assert!(ledger.budget_spent(5_000 + PASS_BUDGET_MS));
    assert!(
      !ledger.budget_spent(4_000),
      "a clock that steps back spends nothing"
    );
  }

  #[test]
  fn the_exclusion_var_only_adds() {
    let extra = "2".repeat(64);
    let ids = excluded_pigeons(Some(&format!(" {extra} ,garbage,{},", FIELD_PIGEONS[1])));
    assert_eq!(ids.len(), 4);
    for id in FIELD_PIGEONS {
      assert!(ids.iter().any(|x| x == id));
    }
    assert!(ids.contains(&extra));
    assert_eq!(excluded_pigeons(Some("garbage")).len(), 3);
  }

  #[test]
  fn a_redelivery_carries_the_same_batch_id() {
    let a = batch_id("p", r#"[{"at_secs":1,"metrics":{"k":"1"}}]"#);
    assert_eq!(a, batch_id("p", r#"[{"at_secs":1,"metrics":{"k":"1"}}]"#));
    assert_ne!(a, batch_id("q", r#"[{"at_secs":1,"metrics":{"k":"1"}}]"#));
    assert_ne!(a, batch_id("p", r#"[{"at_secs":2,"metrics":{"k":"1"}}]"#));
    assert_eq!(a.len(), 64);
    assert!(
      a.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(RuleWork::new("p", "[]", false, Vec::new()).is_none());
  }

  #[test]
  fn a_sentinel_endpoint_token_never_reaches_the_event_or_body() {
    let sentinel = "SENTINEL-TOKEN-5c0e";
    let mut first = reading(T0 - 10, &[("temp", "20")]);
    first.previous = Some(HashMap::from([(
      "temp".to_string(),
      PreviousTelemetryValue {
        value: "19".to_string(),
        reported_at: T0 - 70,
      },
    )]));
    let write = TelemetryWriteResult {
      readings: vec![first, reading(T0, &[("temp", "21")])],
      telemetry_endpoint: Some(capsules::TelemetryEndpoint {
        url: "https://example.invalid/write".to_string(),
        db: Some("db".to_string()),
        auth_token: Some(sentinel.to_string()),
      }),
    };
    // The DO's answer as the consumer receives it, endpoint and all.
    let answer = serde_json::to_string(&write).expect("serializes");
    assert!(answer.contains(sentinel));
    let parsed: TelemetryWriteResult = serde_json::from_str(&answer).expect("parses");
    let payload = r#"[{"at_secs":1793379990,"metrics":{"temp":"20"}}]"#;
    let work = RuleWork::new("p1", payload, false, parsed.readings).expect("has readings");
    let event = build_event(&work, named("f1", "Flock"), named("p1", "Pigeon"), T0);
    let rules = sent(&[("r1", 1), ("r2", 3)]);
    let body = live_input(&event, &rules).expect("serializes");
    assert_eq!(state_total(&rules), 4 * 2, "four empty documents");
    assert!(!body.contains(sentinel));
    assert!(
      !serde_json::to_string(&event)
        .expect("serializes")
        .contains(sentinel)
    );

    // Each rule's event is exactly the contract's: common, plus its own rule and state.
    let wire: Value = serde_json::from_str(&body).expect("parses");
    assert_eq!(wire["mode"], "live");
    assert_eq!(wire["wall_ms"], HARNESS_WALL_MS);
    for rule in wire["rules"].as_array().expect("rules") {
      let mut whole = wire["common"].as_object().cloned().expect("common");
      whole.insert(
        "rule".into(),
        json!({ "id": rule["id"], "version": rule["version"] }),
      );
      whole.insert("state".into(), rule["state"].clone());
      let event: RuleEvent = serde_json::from_value(Value::Object(whole)).expect("contract event");
      assert_eq!(event.readings.len(), 2);
      assert_eq!(event.readings[0].previous["temp"].value, "19");
      assert_eq!(event.readings[0].previous["temp"].at, T0 - 70);
      assert!(event.readings[1].previous.is_empty());
      assert_eq!(event.batch_id, batch_id("p1", payload));
    }

    // A degraded batch hands every reading an empty `previous`.
    let degraded = RuleWork::new("p1", payload, true, work.readings.clone()).expect("readings");
    let event = build_event(&degraded, named("f1", "F"), named("p1", "P"), T0);
    assert!(event.degraded && event.readings.iter().all(|r| r.previous.is_empty()));
  }

  #[test]
  fn every_canned_body_is_classified() {
    let worked = per_rule(classify(&worked_sent(), answered(LIVE_WORKED)));
    assert_eq!(
      statuses(&worked),
      [
        ("r6", RunStatus::Ok),
        ("r7", RunStatus::Ok),
        ("r10", RunStatus::Ok),
        ("gone", RunStatus::VersionMismatch)
      ]
    );
    let r6 = worked[0].result.as_ref().expect("validated");
    assert_eq!(r6.alerts[0].kind, RuleAlertKind::Notify);
    let r10 = worked[2].result.as_ref().expect("validated");
    assert_eq!(
      (r10.alerts[0].kind, r10.alerts[0].scope),
      (RuleAlertKind::Fire, RuleAlertScope::Flock)
    );

    let hostile = per_rule(classify(&hostile_sent(), answered(LIVE_HOSTILE)));
    assert_eq!(
      statuses(&hostile),
      [
        ("big", RunStatus::BadOutput),
        ("ok", RunStatus::Ok),
        ("lonethrow", RunStatus::Threw),
        ("hang", RunStatus::Timeout),
        ("after", RunStatus::Skipped)
      ]
    );
    assert_eq!(hostile[2].error.as_deref(), Some("x\u{fffd}"));
    assert!(
      hostile[0]
        .error
        .as_deref()
        .is_some_and(|e| e.contains("BigInt"))
    );

    let one = sent(&[("r6", 1)]);
    for body in [
      HARNESS_FAILED,
      BAD_INPUT,
      "not json",
      "{\"contract\":1}",
      "[]",
    ] {
      assert_eq!(
        classify(&one, answered(body)),
        Classified::Flock(RunStatus::DispatchFailed),
        "{body}"
      );
    }
    let five_hundred = DispatchOutcome::Answered {
      status: 500,
      body: LIVE_WORKED.as_bytes().to_vec(),
    };
    assert_eq!(
      classify(&worked_sent(), five_hundred),
      Classified::Flock(RunStatus::DispatchFailed)
    );
    // A rejection carries no text: its message may be the rule's own, so a CPU kill, a missing
    // script and a forged "Worker not found." all read alike, and the flock's.
    assert_eq!(
      classify(&one, DispatchOutcome::Failed),
      Classified::Flock(RunStatus::DispatchFailed)
    );
    assert_eq!(
      classify(&one, DispatchOutcome::Binding),
      Classified::Platform
    );
  }

  #[test]
  fn a_response_that_is_not_the_harness_fails_the_whole_dispatch() {
    // Results out of order, short, or for rules not sent: user code can forge the body, so a
    // mismatch is the flock's dispatch_failed rather than a guess at which rule meant what.
    let mut reordered = worked_sent();
    reordered.swap(0, 1);
    let forged = [
      (reordered, LIVE_WORKED),
      (sent(&[("r6", 1), ("r7", 1), ("r10", 1)]), LIVE_WORKED),
      (
        sent(&[("r6", 1)]),
        r#"{"contract":1,"results":[{"id":"r6","status":"ok","logs":[]}]}"#,
      ),
      (
        sent(&[("r6", 1)]),
        r#"{"contract":1,"results":[{"id":"r6","status":"won"}]}"#,
      ),
      (
        sent(&[("r6", 1)]),
        r#"{"contract":2,"results":[{"id":"r6","status":"skipped"}]}"#,
      ),
    ];
    for (rules, body) in forged {
      assert_eq!(
        classify(&rules, answered(body)),
        Classified::Flock(RunStatus::DispatchFailed),
        "{body}"
      );
    }
    // A result the validator refuses is that rule's bad_output, named by place and problem.
    let body =
      r#"{"contract":1,"results":[{"id":"r6","status":"ok","out_json":"{\"shadow\":{}}"}]}"#;
    let records = per_rule(classify(&sent(&[("r6", 1)]), answered(body)));
    assert_eq!(records[0].status, RunStatus::BadOutput);
    assert_eq!(
      records[0].error.as_deref(),
      Some("shadow: not available yet")
    );
  }

  #[test]
  fn caps_hold_on_what_the_harness_returns() {
    // The read cap: one byte over is oversize for the whole dispatch.
    let rules = sent(&[("r6", 1), ("r7", 1)]);
    let cap = read_cap(rules.len());
    assert_eq!(cap, 2 * 103 * 1024 + 1024);
    let over = DispatchOutcome::Answered {
      status: 200,
      body: vec![b' '; cap + 1],
    };
    assert_eq!(
      classify(&rules, over),
      Classified::Flock(RunStatus::Oversize)
    );

    // Logs and errors past their caps are cut, on a character boundary.
    let long_line = "é".repeat(300);
    let logs: Vec<String> = (0..30).map(|_| long_line.clone()).collect();
    let body = json!({ "contract": 1, "results": [
      { "id": "r6", "status": "threw", "error": "ü".repeat(900), "logs": logs },
    ]})
    .to_string();
    let records = per_rule(classify(&sent(&[("r6", 1)]), answered(&body)));
    assert_eq!(records[0].logs.len(), MAX_RULE_LOG_LINES);
    assert!(
      records[0]
        .logs
        .iter()
        .all(|l| l.len() <= MAX_RULE_LOG_LINE_BYTES)
    );
    assert_eq!(records[0].logs[0].len(), 256);
    let error = records[0].error.as_deref().expect("error kept");
    assert_eq!(error.len(), MAX_RULE_ERROR_BYTES);

    // The result cap is the validator's: over 48 KiB is bad_output.
    let huge = json!({ "values": [{ "key": "k", "value": "x".repeat(50_000) }] }).to_string();
    let body =
      json!({ "contract": 1, "results": [{ "id": "r6", "status": "ok", "out_json": huge }]})
        .to_string();
    let records = per_rule(classify(&sent(&[("r6", 1)]), answered(&body)));
    assert_eq!(records[0].status, RunStatus::BadOutput);
    assert_eq!(records[0].error.as_deref(), Some("result: too large"));
  }

  #[test]
  fn a_nul_from_user_code_never_reaches_a_run_row() {
    let body = json!({ "contract": 1, "results": [
      { "id": "r6", "status": "threw", "error": "a\0b".repeat(400),
        "logs": ["x\0y", "\0".repeat(300)] },
      { "id": "r7", "status": "bad_output", "error": "\0" },
    ]})
    .to_string();
    let records = per_rule(classify(&sent(&[("r6", 1), ("r7", 1)]), answered(&body)));
    for record in &records {
      let error = record.error.as_deref().expect("error kept");
      assert!(!error.contains('\0') && error.len() <= MAX_RULE_ERROR_BYTES);
      for line in &record.logs {
        assert!(!line.contains('\0') && line.len() <= MAX_RULE_LOG_LINE_BYTES);
      }
    }
    assert_eq!(records[0].logs[0], "x\u{fffd}y");
    assert_eq!(records[1].error.as_deref(), Some("\u{fffd}"));
  }

  #[test]
  fn a_throwing_rule_never_fails_the_batch() {
    let rules = hostile_sent();
    let classified = classify(&rules, answered(LIVE_HOSTILE));
    let mut ledger = PassLedger::new(0);
    ledger.record("a1", &classified, 0);
    assert_eq!(ledger.may_dispatch("a1", 10), Ok(()));

    let records = per_rule(classified);
    let ok = records.iter().find(|r| r.id == "ok").expect("sibling");
    let result = ok.result.as_ref().expect("validated");
    let state = RuleState::default();
    let flags = AlertFlags::new();
    let mut total = 0;
    let mut counts = MailCounts::default();
    let plan = plan_apply(
      result,
      &ok.logs,
      &ctx(&state, &flags, T0),
      &mut total,
      &mut counts,
    )
    .expect("inside the budget");
    assert_eq!(plan.values.len(), 1);
    assert_eq!(plan.values[0].value_num, Some(1.0));
    for r in records.iter().filter(|r| r.status != RunStatus::Ok) {
      assert!(r.result.is_none());
    }
    // The thrower's streak moves; the healthy sibling's does not.
    let thrower = records
      .iter()
      .find(|r| r.id == "lonethrow")
      .expect("thrower");
    assert!(matches!(
      next_streak(FailStreak::default(), thrower.status, T0),
      StreakWrite::Set(_)
    ));
    assert_eq!(
      next_streak(FailStreak::default(), ok.status, T0),
      StreakWrite::Keep
    );
  }

  #[test]
  fn a_duplicate_batch_writes_nothing_twice() {
    let records = per_rule(classify(&worked_sent(), answered(LIVE_WORKED)));
    for record in records.iter().filter(|r| r.status == RunStatus::Ok) {
      let result = record.result.as_ref().expect("validated");
      let mut store = Store::default();
      let stale = store.snapshot();
      let stale_flags = store.flags.clone();
      store.run(result, T0, T0 + 1);
      let (writes, mails) = (store.writes, store.mails);
      assert!(writes > 0, "{} wrote nothing the first time", record.id);

      // Redelivered after the first committed: decided on the new snapshot.
      let again = store.run(result, T0, T0 + 30);
      assert!(
        again.alerts.is_empty() && again.pigeon_patch.is_none(),
        "{}",
        record.id
      );
      // Redelivered beside the first: decided on the same stale snapshot.
      let racing = store.plan(result, &stale, &stale_flags, T0, T0 + 2);
      assert!(racing.run_row, "{} plans its run row again", record.id);
      store.apply(&T0.to_string(), &racing, T0 + 2);
      assert_eq!(
        (store.writes, store.mails),
        (writes, mails),
        "{}",
        record.id
      );
    }
  }

  #[test]
  fn two_racing_fires_mail_once() {
    for scope in [RuleAlertScope::Pigeon, RuleAlertScope::Flock] {
      let mut store = Store::default();
      let result = RuleResult {
        alerts: vec![alert("hot", RuleAlertKind::Fire, scope)],
        ..RuleResult::default()
      };
      let snapshot = store.snapshot();
      let flags = store.flags.clone();
      let a = store.plan(&result, &snapshot, &flags, T0, T0);
      let b = store.plan(&result, &snapshot, &flags, T0 + 5, T0 + 5);
      assert!(a.alerts[0].mailed && b.alerts[0].mailed);
      store.apply("a", &a, T0);
      store.apply("b", &b, T0 + 5);
      assert_eq!(store.mails, 1);
    }
  }

  #[test]
  fn restating_writes_nothing_so_a_race_can_lose_a_restatement() {
    let pigeon = RuleAlertScope::Pigeon;
    let fire = RuleResult {
      alerts: vec![alert("hot", RuleAlertKind::Fire, pigeon)],
      ..RuleResult::default()
    };
    let clear = RuleResult {
      alerts: vec![alert("hot", RuleAlertKind::Clear, pigeon)],
      ..RuleResult::default()
    };

    // Firing since T0. X (last reading T0 + 10) clears; Y (T0 + 20) restates fire, which plans
    // nothing, so X's clear lands and the next batch fires again.
    let mut store = Store::default();
    store.run(&fire, T0, T0);
    let (state, flags) = (store.snapshot(), store.flags.clone());
    let x = store.plan(&clear, &state, &flags, T0 + 10, T0 + 30);
    let y = store.plan(&fire, &state, &flags, T0 + 20, T0 + 30);
    assert!(y.alerts.is_empty());
    store.apply("y", &y, T0 + 30);
    store.apply("x", &x, T0 + 31);
    store.run(&fire, T0 + 40, T0 + 50);
    assert_eq!(store.mails, 3, "fire, the race's clear, fire again");

    // No row yet. A newer clear restates the default and writes nothing, so an older fire lands.
    let mut store = Store::default();
    store.run(&clear, T0 + 60, T0 + 61);
    store.run(&fire, T0, T0 + 62);
    assert_eq!(store.mails, 1);
  }

  #[test]
  fn a_flock_flag_flips_at_most_once_per_ten_minutes() {
    let flock = RuleAlertScope::Flock;
    let fire = RuleResult {
      alerts: vec![alert("down", RuleAlertKind::Fire, flock)],
      ..RuleResult::default()
    };
    let clear = RuleResult {
      alerts: vec![alert("down", RuleAlertKind::Clear, flock)],
      ..RuleResult::default()
    };
    let mut store = Store::default();
    store.run(&fire, 1_000, 1_000);
    assert_eq!(store.mails, 1);
    store.run(&clear, 1_100, 1_100);
    assert_eq!(store.mails, 1, "inside the hold");
    store.run(&clear, 1_650, 1_700);
    assert_eq!(store.mails, 2, "past the hold");
    store.run(&fire, 1_600, 2_400);
    assert_eq!(store.mails, 2, "an older batch never undoes a newer one");

    // Interleaved batches deciding on snapshots up to four batches stale. Every change is decided
    // on a newer batch than the change before it; a flock flag also holds ten minutes.
    for scope in [flock, RuleAlertScope::Pigeon] {
      let fire = RuleResult {
        alerts: vec![alert("down", RuleAlertKind::Fire, scope)],
        ..RuleResult::default()
      };
      let clear = RuleResult {
        alerts: vec![alert("down", RuleAlertKind::Clear, scope)],
        ..RuleResult::default()
      };
      let mut store = Store::default();
      let mut history: Vec<(RuleState, AlertFlags)> = Vec::new();
      let mut seed: u64 = 0x5eed;
      let mut changes: Vec<(i64, i64)> = Vec::new();
      for step in 0..400_i64 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let now = 10_000 + step * 37;
        let last_reading = now - (seed >> 33) as i64 % 300;
        let stale = (seed >> 40) as usize % 5;
        history.push((store.snapshot(), store.flags.clone()));
        let (state, flags) = &history[history.len().saturating_sub(1 + stale)];
        let result = if (seed >> 50).is_multiple_of(2) {
          &fire
        } else {
          &clear
        };
        let plan = store.plan(result, state, flags, last_reading, now);
        let before = store.flags.get(&(scope, "down".to_string())).copied();
        store.apply(&step.to_string(), &plan, now);
        let after = store.flags.get(&(scope, "down".to_string())).copied();
        if let Some(flag) = after
          && before.is_none_or(|b| b.firing != flag.firing)
        {
          changes.push((now, flag.decided_at));
        }
      }
      assert!(
        changes.len() > 5,
        "{scope:?}: the sequence must actually flip"
      );
      for pair in changes.windows(2) {
        if scope == flock {
          assert!(pair[1].0 - pair[0].0 >= FLOCK_FLAG_HOLD_SECS, "{pair:?}");
        }
        assert!(pair[1].1 > pair[0].1, "{scope:?}: {pair:?}");
      }
    }
  }

  #[test]
  fn notify_mails_once_a_week_per_key() {
    let result = RuleResult {
      alerts: vec![alert(
        "boot-1",
        RuleAlertKind::Notify,
        RuleAlertScope::Pigeon,
      )],
      ..RuleResult::default()
    };
    let mut store = Store::default();
    store.run(&result, T0, T0);
    store.run(&result, T0 + 60, T0 + 60);
    store.run(&result, T0 + 86_400, T0 + 86_400);
    assert_eq!(store.mails, 1);
    store.run(&result, T0 + NOTIFY_REPEAT_SECS, T0 + NOTIFY_REPEAT_SECS);
    assert_eq!(store.mails, 2);
  }

  #[test]
  fn a_quiet_run_plans_zero_statements() {
    let stored = json!({ "seen": 3 })
      .as_object()
      .cloned()
      .unwrap_or_default();
    let state = RuleState {
      pigeon: stored.clone(),
      flock: Map::new(),
    };
    let mut flags = AlertFlags::new();
    flags.insert(
      (RuleAlertScope::Pigeon, "hot".to_string()),
      AlertFlag {
        firing: true,
        decided_at: T0 - 100,
        changed_at: T0 - 100,
        last_sent_at: Some(T0 - 100),
      },
    );
    let mut quiet = RuleResult {
      alerts: vec![alert("hot", RuleAlertKind::Fire, RuleAlertScope::Pigeon)],
      ..RuleResult::default()
    };
    quiet.state.pigeon = Some(stored);
    let mut total = state_doc_bytes(&state.pigeon);
    let mut counts = MailCounts::default();
    let run = |context: &RunContext, total: &mut usize, counts: &mut MailCounts| {
      plan_apply(&quiet, &[], context, total, counts).expect("inside the budget")
    };
    for empty in [&RuleResult::default(), &quiet] {
      let context = ctx(&state, &flags, T0);
      let plan = plan_apply(empty, &[], &context, &mut total, &mut counts).expect("budget");
      assert_eq!(plan.statements(), 0);
      assert_eq!(plan, ApplyPlan::default());
    }
    assert_eq!(
      next_streak(FailStreak::default(), RunStatus::Ok, T0),
      StreakWrite::Keep
    );
    let mut watched = ctx(&state, &flags, T0);
    watched.watched = true;
    assert_eq!(run(&watched, &mut total, &mut counts).statements(), 1);
    // A failing rule's good run writes its reset through the streak, not the plan.
    let failing = FailStreak {
      count: 3,
      since: Some(T0 - 60),
    };
    assert_eq!(next_streak(failing, RunStatus::Ok, T0), StreakWrite::Reset);
    let logged = plan_apply(
      &quiet,
      &["hi".to_string()],
      &ctx(&state, &flags, T0),
      &mut total,
      &mut counts,
    )
    .expect("budget");
    assert!(logged.run_row);
  }

  #[test]
  fn the_state_budget_refuses_growth_past_128_kib() {
    let state = RuleState::default();
    let flags = AlertFlags::new();
    let mut grow = RuleResult::default();
    grow.state.flock = json!({ "k": "x".repeat(2_000) }).as_object().cloned();
    let mut counts = MailCounts::default();

    let mut total = MAX_RULE_STATE_TOTAL_BYTES - 100;
    let refused = plan_apply(
      &grow,
      &[],
      &ctx(&state, &flags, T0),
      &mut total,
      &mut counts,
    );
    assert_eq!(refused, Err(StateBudgetExceeded));
    assert_eq!(StateBudgetExceeded.as_str(), "flock state budget");
    assert_eq!(
      total,
      MAX_RULE_STATE_TOTAL_BYTES - 100,
      "a refusal spends nothing"
    );

    let mut total = 1_000;
    let plan = plan_apply(
      &grow,
      &[],
      &ctx(&state, &flags, T0),
      &mut total,
      &mut counts,
    );
    assert!(plan.is_ok());
    assert_eq!(
      total,
      1_000 - 2 + state_doc_bytes(grow.state.flock.as_ref().unwrap())
    );

    // Already over by a concurrent writer: a patch that shrinks still applies.
    let big = json!({ "k": "x".repeat(2_000) })
      .as_object()
      .cloned()
      .unwrap_or_default();
    let over = RuleState {
      pigeon: Map::new(),
      flock: big,
    };
    let mut shrink = RuleResult::default();
    shrink.state.flock = json!({ "k": null }).as_object().cloned();
    let mut total = MAX_RULE_STATE_TOTAL_BYTES + 500;
    let plan = plan_apply(
      &shrink,
      &[],
      &ctx(&over, &flags, T0),
      &mut total,
      &mut counts,
    );
    assert!(plan.is_ok_and(|p| p.flock_patch.is_some()));
  }

  #[test]
  fn values_are_clamped_into_the_batch_and_kept_once() {
    let state = RuleState::default();
    let flags = AlertFlags::new();
    let mut context = ctx(&state, &flags, T0);
    context.first_reading = T0 - 60;
    let value = |at: Option<i64>| capsules::RuleResultValue {
      key: "k".to_string(),
      value: "1.5".to_string(),
      at,
    };
    let result = RuleResult {
      values: vec![
        value(Some(T0 - 999)),
        value(Some(T0 + 999)),
        value(None),
        value(Some(T0 - 30)),
      ],
      ..RuleResult::default()
    };
    let mut total = 0;
    let plan = plan_apply(
      &result,
      &[],
      &context,
      &mut total,
      &mut MailCounts::default(),
    )
    .expect("budget");
    let ats: Vec<i64> = plan.values.iter().map(|v| v.at).collect();
    assert_eq!(ats, [T0 - 60, T0, T0 - 30]);
    assert!(plan.values.iter().all(|v| v.value_num == Some(1.5)));
    assert_eq!(plan.statements(), 2, "one insert and the run row");
  }

  #[test]
  fn mail_caps_stop_at_their_numbers() {
    let hour = T0 - T0.rem_euclid(3_600);
    let day = T0.div_euclid(86_400);
    let fresh = RuleMailCounters::default();
    let mut counts = MailCounts::at(fresh, 0, 0, T0);
    let sent = (0..15).filter(|_| counts.claim()).count();
    assert_eq!(sent, RULE_MAIL_PER_HOUR as usize);

    let busy_day = RuleMailCounters {
      hour_start: Some(hour - 3_600),
      in_hour: 10,
      day: Some(day),
      in_day: 49,
    };
    let mut counts = MailCounts::at(busy_day, 0, 0, T0);
    assert_eq!(counts.rule_hour, 0, "a new hour starts at zero");
    assert!(counts.claim());
    assert!(!counts.claim(), "50 a day");
    let yesterday = RuleMailCounters {
      day: Some(day - 1),
      in_day: 50,
      ..busy_day
    };
    assert!(MailCounts::at(yesterday, 0, 0, T0).claim());
    assert!(!MailCounts::at(fresh, ACCOUNT_RULE_MAIL_PER_DAY, 0, T0).claim());
    assert!(!MailCounts::at(fresh, 0, PLATFORM_RULE_MAIL_PER_DAY, T0).claim());

    // Past a cap the flag still moves and the mail is suppressed.
    let state = RuleState::default();
    let flags = AlertFlags::new();
    let result = RuleResult {
      alerts: vec![alert("hot", RuleAlertKind::Fire, RuleAlertScope::Pigeon)],
      ..RuleResult::default()
    };
    let mut full = MailCounts::at(fresh, ACCOUNT_RULE_MAIL_PER_DAY, 0, T0);
    let plan =
      plan_apply(&result, &[], &ctx(&state, &flags, T0), &mut 0, &mut full).expect("budget");
    assert!(plan.alerts[0].flag.firing && !plan.alerts[0].mailed);
  }

  #[test]
  fn a_dispatch_failed_streak_under_an_hour_pauses_nothing() {
    let mut streak = FailStreak::default();
    for i in 0..40 {
      match next_streak(streak, RunStatus::DispatchFailed, T0 + i * 60) {
        StreakWrite::Set(next) => streak = next,
        other => panic!("paused inside the hour at run {i}: {other:?}"),
      }
    }
    assert_eq!(streak.since, Some(T0));
    match next_streak(streak, RunStatus::DispatchFailed, T0 + 3_600) {
      StreakWrite::Pause { reason, .. } => assert_eq!(reason, "auto:dispatch_failed"),
      other => panic!("an hour-old streak must pause: {other:?}"),
    }

    // A rule's own failure pauses at the twentieth run, however quickly.
    let mut streak = FailStreak::default();
    for i in 0..19 {
      let StreakWrite::Set(next) = next_streak(streak, RunStatus::Threw, T0 + i) else {
        panic!("paused early");
      };
      streak = next;
    }
    assert!(matches!(
      next_streak(streak, RunStatus::Threw, T0 + 19),
      StreakWrite::Pause { ref reason, .. } if reason == "auto:threw"
    ));
    assert_eq!(next_streak(streak, RunStatus::Ok, T0), StreakWrite::Reset);
    assert_eq!(
      next_streak(streak, RunStatus::Skipped, T0),
      StreakWrite::Keep
    );
    assert_eq!(
      next_streak(streak, RunStatus::VersionMismatch, T0),
      StreakWrite::Keep
    );
    assert_eq!(
      next_streak(FailStreak::default(), RunStatus::Ok, T0),
      StreakWrite::Keep
    );
  }

  #[test]
  fn the_breaker_and_the_pass_budget() {
    let failed = Classified::Flock(RunStatus::DispatchFailed);
    let mut ledger = PassLedger::new(1_000);
    ledger.record("a1", &Classified::PerRule(Vec::new()), 0);
    assert_eq!(ledger.may_dispatch("a1", 1_100), Ok(()));
    ledger.record("a1", &failed, 0);
    // A failed account's every flock waits for the next invocation, so one account with two
    // burning flocks costs other tenants one 2 s wait per invocation, not two.
    assert_eq!(ledger.may_dispatch("a1", 1_100), Err(Skip::AccountFailed));
    assert_eq!(ledger.may_dispatch("a2", 1_100), Ok(()));
    ledger.record("a1", &Classified::Flock(RunStatus::Oversize), 0);
    assert_eq!(
      ledger.may_dispatch("a2", 1_100),
      Ok(()),
      "one account cannot trip it alone"
    );
    ledger.record("a2", &failed, 0);
    assert_eq!(ledger.may_dispatch("a3", 1_100), Err(Skip::Breaker));

    let mut ledger = PassLedger::new(1_000);
    ledger.record("a1", &Classified::Platform, 0);
    assert_eq!(ledger.may_dispatch("a9", 1_100), Err(Skip::Breaker));
    assert!(ledger.tripped());

    // An answer that came back slow skips its account as a failure does, never counted for the
    // breaker: nothing a rule writes can make a dispatch slow on another tenant's account.
    let quiet = Classified::PerRule(Vec::new());
    let mut ledger = PassLedger::new(1_000);
    ledger.record("a1", &quiet, SLOW_DISPATCH_MS - 1);
    assert_eq!(ledger.may_dispatch("a1", 1_100), Ok(()));
    ledger.record("a1", &quiet, SLOW_DISPATCH_MS);
    assert_eq!(ledger.may_dispatch("a1", 1_100), Err(Skip::AccountFailed));
    ledger.record("a2", &quiet, SLOW_DISPATCH_MS);
    ledger.record("a3", &failed, 0);
    assert_eq!(
      ledger.may_dispatch("a4", 1_100),
      Ok(()),
      "two slow, one failed"
    );

    let ledger = PassLedger::new(1_000);
    assert_eq!(
      ledger.may_dispatch("a1", 1_000 + PASS_BUDGET_MS - 1),
      Ok(())
    );
    assert_eq!(
      ledger.may_dispatch("a1", 1_000 + PASS_BUDGET_MS),
      Err(Skip::Budget)
    );
  }

  const RA: Uuid = Uuid::from_u128(0xa);
  const RB: Uuid = Uuid::from_u128(0xb);

  fn live_rule(id: Uuid) -> LiveRule {
    LiveRule {
      id,
      version: 3,
      state: RuleState::default(),
      flags: AlertFlags::new(),
      mail: RuleMailCounters::default(),
      failing: false,
      watched: false,
      pending: false,
    }
  }

  fn lookup(rules: Vec<LiveRule>) -> Lookup {
    Lookup {
      now: T0,
      flock: named("f", "F"),
      pigeon: named("p", "P"),
      account: Uuid::from_u128(0xacc),
      org: Some(Uuid::from_u128(0xacc)),
      uploaded_at: Some(T0 - 86_400),
      leased: false,
      account_mail_day: 0,
      platform_mail_day: 0,
      rules,
    }
  }

  /// The harness's answer for `runs` of (rule, status, out_json or error).
  fn wire(runs: &[(Uuid, &str, &str)]) -> Classified {
    let results: Vec<Value> = runs
      .iter()
      .map(|(id, status, text)| match *status {
        "ok" => json!({ "id": uuid_str(*id), "status": "ok", "out_json": text, "logs": [] }),
        "threw" => json!({ "id": uuid_str(*id), "status": "threw", "error": text, "logs": ["l"] }),
        other => json!({ "id": uuid_str(*id), "status": other, "logs": [] }),
      })
      .collect();
    let body = json!({ "contract": 1, "results": results }).to_string();
    let sent: Vec<DispatchRule> = runs
      .iter()
      .map(|(id, _, _)| DispatchRule {
        id: uuid_str(*id),
        version: 3,
        state: RuleState::default(),
      })
      .collect();
    classify(&sent, answered(&body))
  }

  const FIRE: &str =
    r#"{"alerts":[{"key":"k","kind":"fire","severity":"warning","scope":"pigeon","subject":"s"}]}"#;

  #[test]
  fn each_rule_owns_the_result_in_its_position() {
    let found = lookup(vec![live_rule(RA), live_rule(RB)]);
    let readings = [
      reading(T0 - 30, &[("k", "1")]),
      reading(T0 - 10, &[("k", "2")]),
    ];
    let values = r#"{"values":[{"key":"v","value":"7","at":1}]}"#;
    let plans = plan_record(
      &found,
      &readings,
      &wire(&[(RA, "ok", values), (RB, "threw", "boom")]),
    );
    assert_eq!(plans.len(), 2);
    assert_eq!((plans[0].rule_id, plans[0].status), (RA, RunStatus::Ok));
    let apply = plans[0].apply.as_ref().expect("an ok run applies");
    assert_eq!(apply.values[0].at, T0 - 30, "clamped into the batch");
    assert!(plans[0].run_row && plans[0].writes);
    assert_eq!((plans[1].rule_id, plans[1].status), (RB, RunStatus::Threw));
    assert_eq!(plans[1].error.as_deref(), Some("boom"));
    assert_eq!(plans[1].logs, ["l"]);
    assert!(plans[1].apply.is_none() && plans[1].run_row && plans[1].writes);
    assert_eq!(plans[1].version, 3);
  }

  #[test]
  fn a_quiet_ok_writes_nothing_unless_a_streak_ends() {
    let mut failing = live_rule(RB);
    failing.failing = true;
    let found = lookup(vec![live_rule(RA), failing]);
    let plans = plan_record(
      &found,
      &[reading(T0, &[("k", "1")])],
      &wire(&[(RA, "ok", "{}"), (RB, "ok", "{}")]),
    );
    assert!(!plans[0].writes && !plans[0].run_row);
    assert!(plans[1].writes && !plans[1].run_row, "only the reset");
  }

  #[test]
  fn a_whole_failure_is_every_rules_and_the_platforms_is_nobodys() {
    let found = lookup(vec![live_rule(RA), live_rule(RB)]);
    let one = [reading(T0, &[("k", "1")])];
    let flock = Classified::Flock(RunStatus::DispatchFailed);
    let plans = plan_record(&found, &one, &flock);
    assert_eq!(plans.len(), 2);
    for plan in &plans {
      assert_eq!(plan.status, RunStatus::DispatchFailed);
      assert!(plan.writes && plan.run_row && plan.apply.is_none() && plan.error.is_none());
    }
    assert!(plan_record(&found, &one, &Classified::Platform).is_empty());
    let mut deploying = found.clone();
    deploying.leased = true;
    assert!(
      plan_record(&deploying, &one, &flock).is_empty(),
      "a script our deploy is replacing or deleting fails nobody's rule"
    );
  }

  #[test]
  fn a_result_postgres_refuses_records_as_bad_output_alone() {
    let found = lookup(vec![live_rule(RA)]);
    let values = r#"{"values":[{"key":"v","value":"7"}]}"#;
    let plans = plan_record(
      &found,
      &[reading(T0, &[("k", "1")])],
      &wire(&[(RA, "ok", values)]),
    );
    let refused = plans[0].unstorable();
    assert_eq!(
      (refused.rule_id, refused.version, refused.status),
      (RA, 3, RunStatus::BadOutput)
    );
    assert!(refused.apply.is_none() && refused.logs.is_empty());
    assert!(refused.run_row && refused.writes && refused.error.is_some());
  }

  #[test]
  fn a_mismatch_during_a_deploy_is_quiet() {
    let mut pending = live_rule(RA);
    pending.pending = true;
    let mut found = lookup(vec![pending, live_rule(RB)]);
    let one = [reading(T0, &[("k", "1")])];
    let both = wire(&[(RA, "version_mismatch", ""), (RB, "version_mismatch", "")]);
    let plans = plan_record(&found, &one, &both);
    assert!(!plans[0].writes, "a pending version is a deploy in flight");
    assert!(
      plans[1].run_row,
      "a day-old upload holding another version is not"
    );
    found.leased = true;
    assert!(plan_record(&found, &one, &both).iter().all(|p| !p.writes));
    found.leased = false;
    found.uploaded_at = Some(T0 - MISMATCH_ROW_AFTER_SECS + 1);
    assert!(!plan_record(&found, &one, &both)[1].writes);
  }

  #[test]
  fn mail_counts_carry_from_rule_to_rule() {
    let mut found = lookup(vec![live_rule(RA), live_rule(RB)]);
    found.account_mail_day = ACCOUNT_RULE_MAIL_PER_DAY - 1;
    let plans = plan_record(
      &found,
      &[reading(T0, &[("k", "1")])],
      &wire(&[(RA, "ok", FIRE), (RB, "ok", FIRE)]),
    );
    let mailed = |p: &RulePlan| p.apply.as_ref().expect("applied").alerts[0].mailed;
    assert!(mailed(&plans[0]));
    assert!(
      !mailed(&plans[1]),
      "the account's last mail went to the first rule"
    );
  }

  #[test]
  fn rule_mail_goes_only_to_addresses_the_account_holds() {
    let to = [
      "Ops@Example.com".to_string(),
      "stranger@else.org".to_string(),
    ];
    let (sent, refused) = deliverable_recipients(
      &to,
      Some("owner@example.com"),
      Some("a@x.io,ops@example.com"),
    );
    assert_eq!((sent, refused), (vec!["Ops@Example.com".to_string()], 1));
    let (sent, refused) = deliverable_recipients(&to[1..], Some("owner@example.com"), None);
    assert_eq!((sent, refused), (vec!["owner@example.com".to_string()], 1));
    assert_eq!(
      deliverable_recipients(&[], None, Some("a@x.io")),
      (Vec::new(), 0)
    );
  }

  #[test]
  fn rule_mail_carries_a_fixed_prefix_and_names_its_rule() {
    let mut fire = alert("down", RuleAlertKind::Fire, RuleAlertScope::Pigeon);
    fire.subject = "Stop 9 is dark".to_string();
    fire.body = Some("No frame since 9:40.".to_string());
    let (subject, text) = rule_mail_message(&fire, "Dark signs", "Holyoke", "Stop 9");
    assert_eq!(subject, "PidgeIoT rule: Stop 9 is dark");
    assert!(text.starts_with("No frame since 9:40.\n\n"));
    for line in [
      "Rule: Dark signs",
      "Flock: Holyoke",
      "Pigeon: Stop 9",
      "Severity: warning",
    ] {
      assert!(text.contains(line), "{line}");
    }
    fire.body = None;
    let (_, text) = rule_mail_message(&fire, "r", "f", "p");
    assert!(text.starts_with("--\n"));
  }

  #[test]
  fn which_failures_keep_a_run_row() {
    assert!(failure_keeps_row(RunStatus::Threw, false, 0));
    assert!(failure_keeps_row(RunStatus::DispatchFailed, false, 0));
    assert!(!failure_keeps_row(RunStatus::Skipped, false, 0));
    assert!(failure_keeps_row(RunStatus::Skipped, true, 0));
    assert!(!failure_keeps_row(RunStatus::VersionMismatch, true, 599));
    assert!(failure_keeps_row(RunStatus::VersionMismatch, false, 600));
    for status in [
      RunStatus::Skipped,
      RunStatus::VersionMismatch,
      RunStatus::Ok,
    ] {
      assert!(!status.counts_toward_pause());
    }
    assert_eq!(RunStatus::VersionMismatch.as_str(), "version_mismatch");
  }

  const RULE_A: &str = "0b5e7c1a-2d3f-4a5b-8c6d-7e8f9a0b1c2d";
  const RULE_B: &str = "9f8e7d6c-5b4a-4321-8fed-cba987654321";
  const FLOCK: &str = "11111111-2222-4333-8444-555555555555";
  const ACCOUNT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

  #[test]
  fn script_names_come_only_from_uuids() {
    assert_eq!(
      script_name(ScriptKind::Flock, FLOCK).as_deref(),
      Some("flock-11111111222243338444555555555555")
    );
    assert_eq!(
      script_name(ScriptKind::Test, RULE_A).as_deref(),
      Some("test-0b5e7c1a2d3f4a5b8c6d7e8f9a0b1c2d")
    );
    for bad in [
      "",
      "dovecote",
      "../x",
      &RULE_A.to_uppercase(),
      &FLOCK.replace('-', ""),
    ] {
      assert_eq!(script_name(ScriptKind::Flock, bad), None, "{bad}");
    }
  }

  /// The builder fixtures `rules/run-builder.mjs` runs, by file: each model's module and the
  /// test cases made from the first, as the test route expands them.
  fn builder_fixtures() -> Vec<(&'static str, capsules::RuleModel)> {
    let model = |v: Value| serde_json::from_value::<capsules::RuleModel>(v).expect("a model");
    vec![
      (
        "b-hot.js",
        model(json!({
          "v": 1,
          "when": {"telemetry": {"key": "temp_c"}},
          "only_if": [{"is": "compare", "key": "temp_c", "op": "above", "to": {"number": 40},
            "for_secs": 300}],
          "then": [
            {"do": "mail", "severity": "warning",
              "subject": "Sign running hot: {pigeon} at {temp_c} C"},
            {"do": "set_value", "key": "fan", "value": "on"}
          ],
          "otherwise": [{"do": "set_value", "key": "fan", "value": "auto"}]
        })),
      ),
      (
        "b-reboots.js",
        model(json!({
          "v": 1,
          "when": {"telemetry": {"key": "reset_cause"}},
          "only_if": [{"is": "counted", "key": "reset_cause", "more_than": 3,
            "within_secs": 86400}],
          "then": [{"do": "mail", "severity": "critical", "subject": "Reboot loop on {pigeon}",
            "body": "{pigeon} booted more than three times in a day."}]
        })),
      ),
      (
        "b-bands.js",
        model(json!({
          "v": 1,
          "when": {"telemetry": {"key": "lux"}},
          "then": [{"do": "set_value", "key": "brightness", "value": "60", "bands": [
            {"if": {"is": "compare", "key": "lux", "op": "below", "to": {"number": 50}},
              "value": "30"},
            {"if": {"is": "compare", "key": "lux", "op": "above", "to": {"number": 500}},
              "value": "80"}
          ]}]
        })),
      ),
      (
        "b-mixed.js",
        model(json!({
          "v": 1,
          "when": {"telemetry": {"key": "reset_cause"}},
          "match": "any",
          "only_if": [
            {"is": "bit_set", "key": "reset_cause", "bit": 4},
            {"is": "changed", "key": "layout"},
            {"is": "changed_by", "key": "temp_c", "more_than": 5},
            {"is": "compare", "key": "mode", "op": "not_equals", "to": {"text": "auto"}},
            {"is": "compare", "key": "temp_in", "op": "above", "to": {"key": "temp_out"}}
          ],
          "then": [{"do": "mail", "severity": "warning", "subject": "Look at {pigeon}",
            "body": "reset_cause {reset_cause}\nlayout {layout} {unknown}", "repeat": "once"}],
          "otherwise": [{"do": "set_value", "key": "last_cause", "value": "{reset_cause}"}]
        })),
      ),
      (
        "b-counted.js",
        model(json!({
          "v": 1,
          "when": {"telemetry": {"key": "temp_c"}},
          "only_if": [{"is": "counted", "key": "reset_cause", "more_than": 2,
            "within_secs": 86400}],
          "then": [{"do": "mail", "severity": "warning", "subject": "Rebooting: {pigeon}"}]
        })),
      ),
    ]
  }

  /// `BLESS_BUILDER_FIXTURES=1` rewrites the files instead of comparing them, after a deliberate
  /// change to the compile or the case maker.
  #[test]
  fn the_builder_fixtures_are_what_dovecote_writes() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/rules/");
    let bless = std::env::var_os("BLESS_BUILDER_FIXTURES").is_some();
    let fixtures = builder_fixtures();
    for (file, model) in &fixtures {
      assert_eq!(capsules::validate_rule_model(model), Ok(()), "{file}");
      let source = capsules::compile_rule_model(model);
      let path = [dir, file].concat();
      if bless {
        std::fs::write(&path, &source).expect("fixture written");
      }
      assert_eq!(
        std::fs::read_to_string(&path).expect("fixture read"),
        source,
        "{file}"
      );
    }
    let flock = RuleNamedRef {
      id: "f1".into(),
      name: "Bench signs".into(),
    };
    let cases: Vec<Value> = capsules::rule_model_test_cases(&fixtures[0].1)
      .iter()
      .map(|case| {
        let events = expand_case(case, &flock, 1_793_380_000).expect("a valid case");
        json!({ "name": case.name, "events": events, "expect": case.expect })
      })
      .collect();
    assert_eq!(cases.len(), MIN_TEST_CASES);
    let text = serde_json::to_string_pretty(&cases).expect("cases serialize") + "\n";
    let path = [dir, "canned/builder-cases.json"].concat();
    if bless {
      std::fs::write(&path, &text).expect("cases written");
    }
    assert_eq!(std::fs::read_to_string(&path).expect("cases read"), text);
  }

  #[test]
  fn the_bundle_holds_the_harness_and_each_rule() {
    let rules = [
      BundleRule {
        key: RULE_A,
        version: 3,
        source: "export default () => ({});\n",
      },
      BundleRule {
        key: RULE_B,
        version: 12,
        source: "export default (e) => ({ values: [] });\n",
      },
    ];
    let manifest = manifest(&rules).expect("valid keys");
    assert_eq!(manifest.len(), manifest.capacity());
    assert!(manifest.contains(&format!("import r0 from \"./r-{RULE_A}.js\";\n")));
    assert!(manifest.contains(&format!("  \"{RULE_B}\": {{ version: 12, run: r1 }},\n")));
    assert!(manifest.contains("export default {\n"));

    let modules = bundle_modules(&rules).expect("valid keys");
    let names: Vec<&str> = modules.iter().map(|(n, _)| n.as_str()).collect();
    let a = format!("r-{RULE_A}.js");
    let b = format!("r-{RULE_B}.js");
    assert_eq!(
      names,
      [
        "main.js",
        "prelude.js",
        "pidgeiot.js",
        "builder.js",
        "rules.js",
        &a,
        &b
      ]
    );
    assert_eq!(modules[0].1, include_str!("../../rules/main.js"));
    assert_eq!(modules[3].1, include_str!("../../rules/builder.js"));
    assert_eq!(modules[4].1, manifest);

    // A dry run carries two versions of one rule under distinct keys.
    let test_key = format!("{RULE_A}-v4");
    let pair = [
      BundleRule {
        key: RULE_A,
        version: 3,
        source: "",
      },
      BundleRule {
        key: &test_key,
        version: 4,
        source: "",
      },
    ];
    assert!(manifest_ok(&pair));
    let twice = [rules[0], rules[0]];
    assert!(!manifest_ok(&twice));
    for key in [
      "r6",
      "\";alert(1);//",
      &RULE_A.to_uppercase(),
      &format!("{RULE_A}-v"),
      &format!("{RULE_A}-vx"),
    ] {
      let bad = [BundleRule {
        key,
        version: 1,
        source: "",
      }];
      assert!(!manifest_ok(&bad), "{key}");
    }
    let unversioned = [BundleRule {
      key: RULE_A,
      version: 0,
      source: "",
    }];
    assert!(!manifest_ok(&unversioned));
  }

  fn manifest_ok(rules: &[BundleRule]) -> bool {
    manifest(rules).is_some() && bundle_modules(rules).is_some()
  }

  #[test]
  fn upload_metadata_states_every_limit() {
    let live: Value = serde_json::from_str(
      &upload_metadata(ScriptKind::Flock, ACCOUNT, FLOCK, RulesEnv::Production).expect("uuids"),
    )
    .expect("JSON");
    assert_eq!(live["main_module"], "main.js");
    assert_eq!(live["compatibility_date"], RULES_COMPATIBILITY_DATE);
    assert_eq!(live["bindings"], json!([]));
    assert_eq!(live["limits"], json!({ "cpu_ms": 50, "subrequests": 0 }));
    assert_eq!(live["observability"], json!({ "enabled": false }));
    assert_eq!(
      live["tags"],
      json!([
        "pidgeiot-rule",
        format!("acct-{ACCOUNT}"),
        format!("flock-{FLOCK}"),
        "env-prod"
      ])
    );
    let keys: Vec<&String> = live.as_object().expect("object").keys().collect();
    assert_eq!(keys.len(), 6, "nothing inherited: {keys:?}");

    let test: Value = serde_json::from_str(
      &upload_metadata(ScriptKind::Test, ACCOUNT, RULE_A, RulesEnv::Staging).expect("uuids"),
    )
    .expect("JSON");
    assert_eq!(test["limits"]["cpu_ms"], 2_000);
    assert_eq!(
      test["tags"],
      json!([
        "pidgeiot-rule",
        format!("acct-{ACCOUNT}"),
        "test",
        "env-staging"
      ])
    );
    assert!(upload_metadata(ScriptKind::Flock, "x", FLOCK, RulesEnv::Staging).is_none());

    let cpu: Value = serde_json::from_str(
      &upload_metadata(ScriptKind::CpuCheck, ACCOUNT, RULE_A, RulesEnv::Staging).expect("uuids"),
    )
    .expect("JSON");
    assert_eq!(cpu["limits"]["cpu_ms"], RULE_CPU_MS, "the live limit");
    assert_eq!(cpu["tags"], test["tags"], "swept as a test script");
    assert_eq!(
      script_name(ScriptKind::CpuCheck, RULE_A),
      script_name(ScriptKind::Test, RULE_A),
      "it replaces the dry run's own script, so one delete removes either"
    );
  }

  #[test]
  fn a_paused_rule_mails_its_author_the_status_and_error() {
    let (subject, text) = rule_paused_message("Boots", "Bench", "auto:threw", Some("TypeError: x"));
    assert_eq!(subject, "PidgeIoT rule paused: Boots");
    assert!(
      text.starts_with("This rule failed 20 runs in a row, the last as threw, and is paused")
    );
    assert!(text.contains("\n\nIts last error:\nTypeError: x\n\n--\nRule: Boots\nFlock: Bench"));
    let (_, quiet) = rule_paused_message("Boots", "Bench", "auto:dispatch_failed", None);
    assert!(quiet.contains("the last as dispatch_failed") && !quiet.contains("Its last error"));
  }

  #[test]
  fn the_multipart_body_is_sized_and_framed() {
    let boundary = "0123456789abcdef0123456789abcdef";
    let modules = vec![
      ("main.js".to_string(), "export default {};\n".to_string()),
      ("rules.js".to_string(), "export default {};".to_string()),
    ];
    let body = multipart_body(boundary, "{\"main_module\":\"main.js\"}", &modules).expect("valid");
    assert_eq!(body.len(), body.capacity());
    assert!(body.starts_with(&format!("--{boundary}\r\n")));
    assert!(body.ends_with(&format!("\r\n--{boundary}--\r\n")));
    assert_eq!(body.matches(&format!("--{boundary}")).count(), 4);
    assert!(body.contains(
      "Content-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n\
     {\"main_module\":\"main.js\"}\r\n"
    ));
    assert!(body.contains(
      "Content-Disposition: form-data; name=\"rules.js\"; filename=\"rules.js\"\r\n\
     Content-Type: application/javascript+module\r\n\r\nexport default {};\r\n"
    ));
    assert_eq!(
      multipart_content_type(boundary),
      format!("multipart/form-data; boundary={boundary}")
    );

    let inside = vec![("r.js".to_string(), format!("// {boundary}"))];
    assert!(multipart_body(boundary, "{}", &inside).is_none());
    assert!(multipart_body("short", "{}", &modules).is_none());
    assert!(multipart_body(&boundary.to_uppercase(), "{}", &modules).is_none());
    let bad_name = vec![("a\"b.js".to_string(), String::new())];
    assert!(multipart_body(boundary, "{}", &bad_name).is_none());
  }

  fn point(pigeon: &str, key: &str, value: &str, at: i64) -> capsules::TelemetryHistoryPoint {
    capsules::TelemetryHistoryPoint {
      pigeon_id: pigeon.to_string(),
      key: key.to_string(),
      value: value.to_string(),
      value_num: value.parse().ok(),
      reported_at: OffsetDateTime::from_unix_timestamp(at).expect("in range"),
    }
  }

  #[test]
  fn history_regroups_into_contract_batches() {
    let flock = named(FLOCK, "Bench");
    let names = HashMap::from([("pa".to_string(), "Sign A".to_string())]);
    let points = vec![
      point("pa", "temp", "20", T0),
      point("pa", "hum", "40", T0),
      point("pb", "temp", "5", T0 + 5),
      point("pa", "temp", "21", T0 + 10),
    ];
    let events = regroup_history(&points, &flock, &names);
    assert_eq!(events.len(), 2);
    let a = events.iter().find(|e| e.pigeon.id == "pa").expect("pa");
    assert_eq!(a.pigeon.name, "Sign A");
    assert_eq!(a.readings.len(), 2, "one reading per timestamp");
    assert_eq!(a.readings[0].metrics.len(), 2);
    assert!(a.readings[0].previous.is_empty());
    assert_eq!(
      a.readings[1].previous["temp"],
      capsules::RulePrevious {
        value: "20".to_string(),
        at: T0
      }
    );
    assert!(!a.readings[1].previous.contains_key("hum"));
    assert_eq!(a.received_at, T0 + 10);
    assert!(!a.degraded);
    assert_eq!(events[0].received_at, T0 + 5, "events run in time order");
    assert_eq!(
      regroup_history(&points, &flock, &names),
      events,
      "stable batch ids"
    );

    // Long histories: batches of at most 64, the newest 4,000 readings kept, previous intact.
    let long: Vec<_> = (0..4_100)
      .map(|i| point("pa", "n", &i.to_string(), T0 + i))
      .collect();
    let events = regroup_history(&long, &flock, &names);
    assert!(events.iter().all(|e| (1..=64).contains(&e.readings.len())));
    let total: usize = events.iter().map(|e| e.readings.len()).sum();
    assert_eq!(total, REPLAY_MAX_READINGS);
    assert_eq!(events[0].readings[0].metrics["n"], "100");
    assert_eq!(events[0].readings[0].previous["n"].value, "99");
    let ids: HashSet<&str> = events.iter().map(|e| e.batch_id.as_str()).collect();
    assert_eq!(ids.len(), events.len());
    for event in &events {
      let mut whole = serde_json::to_value(event).expect("serializes");
      whole["rule"] = json!({ "id": "r", "version": 1 });
      whole["state"] = json!({ "pigeon": {}, "flock": {} });
      serde_json::from_value::<RuleEvent>(whole).expect("contract event");
    }
  }

  fn case(json: Value) -> RuleTestCase {
    serde_json::from_value(json).expect("case shape")
  }

  #[test]
  fn the_case_expander_builds_contract_events() {
    let flock = named(FLOCK, "Bench");
    let c = case(json!({
      "name": "two boots",
      "events": [
        { "pigeon": "a", "readings": [
          { "after_secs": 60, "metrics": [{ "key": "uptime_s", "value": "5" }] },
          { "after_secs": 0, "metrics": [{ "key": "uptime_s", "value": "900" },
                                          { "key": "reset_cause", "value": "16" }] },
        ]},
        { "pigeon": "b", "readings": [{ "after_secs": 30, "metrics": [] }] },
        { "pigeon": "a", "readings": [{ "after_secs": 120, "metrics": [
          { "key": "uptime_s", "value": "65" }] }] },
      ],
      "expect": [{ "key": "watchdog", "kind": "notify" }],
    }));
    let events = expand_case(&c, &flock, T0).expect("valid case");
    assert_eq!(events.len(), 3);
    let first = &events[0];
    assert_eq!(first.readings[0].at, T0, "readings sorted oldest first");
    assert_eq!(first.readings[1].at, T0 + 60);
    assert_eq!(first.readings[0].metrics["reset_cause"], "16");
    assert_eq!(first.readings[1].previous["uptime_s"].value, "900");
    assert_eq!(events[2].readings[0].previous["uptime_s"].value, "5");
    assert_eq!(events[2].readings[0].previous["uptime_s"].at, T0 + 60);
    assert_eq!(first.pigeon.id, events[2].pigeon.id);
    assert_ne!(first.pigeon.id, events[1].pigeon.id);
    assert_eq!(first.received_at, T0 + 60);
    assert!(!first.degraded);
    assert!(
      events
        .iter()
        .all(|e| !FIELD_PIGEONS.contains(&e.pigeon.id.as_str()))
    );
    assert_eq!(c.expect[0].kind, RuleAlertKind::Notify);
    for event in &events {
      let mut whole = serde_json::to_value(event).expect("serializes");
      whole["rule"] = json!({ "id": "r", "version": 1 });
      whole["state"] = json!({ "pigeon": {}, "flock": {} });
      serde_json::from_value::<RuleEvent>(whole).expect("contract event");
    }

    let refused = |events: Value| {
      expand_case(
        &case(json!({ "name": "x", "events": events, "expect": [] })),
        &flock,
        T0,
      )
    };
    let one = |metrics: Value| {
      json!([{ "pigeon": "a", "readings": [
    { "after_secs": 0, "metrics": metrics }] }])
    };
    assert_eq!(
      refused(one(
        json!([{ "key": "k", "value": "1" }, { "key": "k", "value": "2" }])
      )),
      Err(CaseError::RepeatedKey)
    );
    assert_eq!(
      refused(one(json!([{ "key": "", "value": "1" }]))),
      Err(CaseError::BadKey)
    );
    let long_value = "v".repeat(1_025);
    assert_eq!(
      refused(one(json!([{ "key": "k", "value": long_value }]))),
      Err(CaseError::BadValue)
    );
    assert_eq!(refused(json!([])), Err(CaseError::NoEvents));
    assert_eq!(
      refused(json!(vec![json!({ "pigeon": "a", "readings": [] })])),
      Err(CaseError::NoReadings)
    );
    let nine: Vec<Value> = (0..9)
      .map(|_| json!({ "pigeon": "a", "readings": [{ "after_secs": 0, "metrics": [] }] }))
      .collect();
    assert_eq!(refused(json!(nine)), Err(CaseError::TooManyEvents));
    let late = json!([{ "pigeon": "a", "readings": [{ "after_secs": -1, "metrics": [] }] }]);
    assert_eq!(refused(late), Err(CaseError::Offset));
    let many: Vec<Value> = (0..65)
      .map(|i| json!({ "after_secs": i, "metrics": [] }))
      .collect();
    assert_eq!(
      refused(json!([{ "pigeon": "a", "readings": many }])),
      Err(CaseError::TooManyReadings)
    );
    assert!(
      serde_json::from_value::<RuleTestCase>(json!({
    "name": "x", "events": [{ "pigeon": "d", "readings": [] }], "expect": [] }))
      .is_err()
    );

    assert_eq!(check_case_count(1), Err(CaseError::CaseCount));
    assert_eq!(check_case_count(2), Ok(()));
    assert_eq!(check_case_count(5), Ok(()));
    assert_eq!(check_case_count(6), Err(CaseError::CaseCount));
    assert_eq!(
      CaseError::RepeatedKey.as_str(),
      "a key repeated in one reading"
    );
  }

  /// Each table of a wrangler config as its header and its `key = value` pairs. A value a
  /// formatter wrapped over several lines is joined back into its one-line form.
  /// Enough TOML for the checks below, which fail closed on any form it does not read.
  fn toml_tables(config: &str) -> Vec<(&str, Vec<(&str, String)>)> {
    let mut tables: Vec<(&str, Vec<(&str, String)>)> = vec![("", Vec::new())];
    let mut open = 0;
    for line in config.lines().map(str::trim) {
      if open > 0 {
        if line.is_empty() || line.starts_with('#') {
          continue;
        }
        let value = &mut tables.last_mut().unwrap().1.last_mut().unwrap().1;
        if line.starts_with(']') {
          if value.ends_with(',') {
            value.pop();
          }
        } else if !value.ends_with('[') {
          value.push(' ');
        }
        value.push_str(line);
        open += toml_depth(line);
      } else if line.starts_with('[') {
        tables.push((line, Vec::new()));
      } else if let Some((key, value)) = line.split_once('=') {
        if !line.starts_with('#') {
          let value = value.trim();
          open = toml_depth(value);
          tables
            .last_mut()
            .unwrap()
            .1
            .push((key.trim(), value.to_string()));
        }
      }
    }
    tables
  }

  /// The brackets and braces `text` opens less those it closes, outside strings and comments.
  fn toml_depth(text: &str) -> i32 {
    let mut depth = 0;
    let mut quoted = false;
    for c in text.chars() {
      match c {
        '"' => quoted = !quoted,
        '#' if !quoted => break,
        '[' | '{' if !quoted => depth += 1,
        ']' | '}' if !quoted => depth -= 1,
        _ => {}
      }
    }
    depth
  }

  /// The value of `key` in one table, raw (quotes kept).
  fn toml_value<'a>(table: &'a [(&str, String)], key: &str) -> Option<&'a str> {
    table
      .iter()
      .find(|(k, _)| *k == key)
      .map(|(_, v)| v.as_str())
  }

  // A namespace binding without `outbound` gives rules the open internet, and production has no
  // outbound Worker yet, so only staging may bind one. The upload path finds the namespace
  // through the same environment's vars, so they must name the bound one.
  #[test]
  fn rules_binding_always_carries_the_outbound_worker() {
    let config = include_str!("../../wrangler.toml");
    let outbound = include_str!("../../rules-outbound/wrangler.toml");
    let outbound_tables = toml_tables(outbound);
    let outbound_name = toml_value(&outbound_tables[0].1, "name").unwrap();
    let tables = toml_tables(config);
    for line in config
      .lines()
      .map(str::trim)
      .filter(|l| !l.starts_with('#'))
    {
      if line.contains("dispatch_namespaces") {
        assert!(
          line.starts_with("[[") && line.ends_with("dispatch_namespaces]]"),
          "{line}"
        );
      }
    }
    let mut bound = Vec::new();
    for (header, table) in &tables {
      let Some(env) = header
        .strip_prefix("[[")
        .and_then(|h| h.strip_suffix("dispatch_namespaces]]"))
      else {
        continue;
      };
      let env = env.trim_end_matches('.');
      assert_eq!(toml_value(table, "binding"), Some("\"RULES\""), "{header}");
      let expected = concat2(
        &concat2("{ service = \"", outbound_name.trim_matches('"')),
        "\", parameters = [\"flock_id\"] }",
      );
      assert_eq!(
        toml_value(table, "outbound"),
        Some(expected.as_str()),
        "{header}"
      );
      let namespace = toml_value(table, "namespace").expect("namespace missing");
      assert_eq!(namespace, "\"dovecote-rules-staging\"", "{header}");
      let vars_header = if env.is_empty() {
        "[vars]".to_string()
      } else {
        concat2(&concat2("[", env), ".vars]")
      };
      let (_, vars) = tables
        .iter()
        .find(|(h, _)| *h == vars_header)
        .expect("the binding's environment has no vars table");
      assert_eq!(
        toml_value(vars, "RULES_NAMESPACE"),
        Some(namespace),
        "{header}"
      );
      let account = toml_value(vars, "RULES_ACCOUNT_ID")
        .expect("RULES_ACCOUNT_ID missing")
        .trim_matches('"');
      assert!(
        account.len() == 32 && account.bytes().all(|b| b.is_ascii_hexdigit()),
        "{header}"
      );
      bound.push(env);
    }
    assert_eq!(bound, ["env.staging"]);
    assert_eq!(outbound_name, "\"dovecote-rules-outbound-staging\"");
  }

  // The Cloudflare budget is the owner user's, so each environment that binds `RULES` binds the
  // limiter on its own id of the 1016/1017/1018 triple, and no other: a production `RULES`
  // without it would deploy with nothing in front of Cloudflare's five-minute block. 25 per 10 s
  // is 150 a minute on a window short enough to bind.
  #[test]
  fn cf_api_limiter_goes_wherever_rules_go_on_its_own_namespace() {
    use crate::helpers::cf_api::{CF_API_LIMITER, CF_API_WINDOW_SECS};
    let tables = toml_tables(include_str!("../../wrangler.toml"));
    let quoted = concat2(&concat2("\"", CF_API_LIMITER), "\"");
    let triple = [
      ("", "\"1016\""),
      ("env.staging", "\"1017\""),
      ("env.dev", "\"1018\""),
    ];
    let simple = concat2(
      &concat2("{ limit = 25, period = ", &CF_API_WINDOW_SECS.to_string()),
      " }",
    );
    let mut ids = Vec::new();
    let mut limited = Vec::new();
    let mut rules = Vec::new();
    for (header, table) in &tables {
      if let Some(env) = header
        .strip_prefix("[[")
        .and_then(|h| h.strip_suffix("dispatch_namespaces]]"))
      {
        rules.push(env.trim_end_matches('.'));
        continue;
      }
      let Some(env) = header
        .strip_prefix("[[")
        .and_then(|h| h.strip_suffix("ratelimits]]"))
      else {
        continue;
      };
      let env = env.trim_end_matches('.');
      let id = toml_value(table, "namespace_id").expect("namespace_id missing");
      assert!(!ids.contains(&id), "{header} reuses {id}");
      ids.push(id);
      let held = triple.iter().find(|(_, held)| *held == id);
      if toml_value(table, "name") == Some(quoted.as_str()) {
        assert_eq!(
          held.map(|(e, _)| *e),
          Some(env),
          "{header} is off its triple id"
        );
        assert_eq!(
          toml_value(table, "simple"),
          Some(simple.as_str()),
          "{header}"
        );
        limited.push(env);
      } else {
        assert_eq!(held, None, "{header} takes {id}, held for {CF_API_LIMITER}");
      }
    }
    assert_eq!(
      limited, rules,
      "the environments binding RULES and {CF_API_LIMITER} differ"
    );
    assert_eq!(rules, ["env.staging"]);
  }
}
