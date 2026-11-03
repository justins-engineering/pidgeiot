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
  MAX_TELEMETRY_VALUE_BYTES, RULE_CONTRACT, RuleAlert, RuleAlertKind, RuleAlertScope, RuleHook,
  RuleNamedRef, RulePrevious, RuleReading, RuleResult, RuleState, TelemetryHistoryPoint,
  merge_state, state_doc_bytes, validate_result,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

/// The production pigeons of signs 414, 73 and 1670, by full id. No rule reads or writes under
/// them until the owner rules otherwise, and they are matched whole, never by prefix.
pub const FIELD_PIGEONS: [&str; 3] = [
  "fa8a80551f1818392b2929f8dc4cf0efb05155e7b7316692ce944eae6ce7a33f",
  "ae3a6d3f63a5231601c416510612fa330d9339ccac86427c6adb93321dd3edc7",
  "c8bd5d567beef36d185bcbf6e8625c2c8a6a755d387ae8a513ee88d779c299f2",
];

/// One budget of wall time for a whole live dispatch, all rules together, kept by the harness.
pub const HARNESS_WALL_MS: u32 = 250;

/// CPU per live dispatch, shared by the bundle; set per call and in the upload metadata.
pub const RULE_CPU_MS: u32 = 50;

/// CPU for a dry run's test script, which replays days of telemetry in one call.
pub const TEST_RULE_CPU_MS: u32 = 2_000;

/// The most one rule's entry takes in a response: its result escaped twice, its logs, framing
/// (`dovecote/rules/wire.d.ts`).
pub const RULE_WIRE_BYTES: usize = 103 * 1024;

/// No new dispatch starts this long after the second pass began, bounding the consumer slot and
/// Hyperdrive time the pass holds.
pub const PASS_BUDGET_MS: u64 = 3_000;

/// Consecutive failing runs that auto-pause a rule.
pub const AUTO_PAUSE_RUNS: i32 = 20;

/// How old a `dispatch_failed` streak must also be before it pauses, so a runtime outage shorter
/// than this pauses nothing while a rule that breaks every dispatch stops within it.
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
/// tested under this one. `rules/run-workerd.mjs` reads this declaration's text, so it stays
/// one `&str` literal.
pub const RULES_COMPATIBILITY_DATE: &str = "2026-10-26";

/// Bounds on the assistant's synthetic test cases, which structured outputs do not enforce.
pub const MIN_TEST_CASES: usize = 2;
/// See [`MIN_TEST_CASES`].
pub const MAX_TEST_CASES: usize = 5;
/// See [`MIN_TEST_CASES`].
pub const MAX_CASE_EVENTS: usize = 8;
/// Furthest a case reading may sit from the case's start: the notify window.
pub const MAX_CASE_OFFSET_SECS: i64 = NOTIFY_REPEAT_SECS;

/// The harness modules every bundle carries beside its rules.
const HARNESS: [(&str, &str); 3] = [
  ("main.js", include_str!("../../rules/main.js")),
  ("prelude.js", include_str!("../../rules/prelude.js")),
  ("pidgeiot.js", include_str!("../../rules/pidgeiot.js")),
];

fn is_pigeon_id(s: &str) -> bool {
  s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_lower_hex(s: &str) -> bool {
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
}

impl PassVars {
  /// [`pre_gate`] for one batch with the switch known to be on.
  pub fn gate(&self, pigeon_id: &str) -> PreGate {
    pre_gate(pigeon_id, Some("on"), &self.excluded)
  }
}

/// Reads `RULES_DISPATCH` through `read` and, only when it is `"on"`, `RULES_EXCLUDED_PIGEONS`.
/// `None` means rules are off and nothing else was read, so an unset switch costs the consumer
/// no binding, statement or dispatch.
pub fn pass_vars(read: impl Fn(&str) -> Option<String>) -> Option<PassVars> {
  let dispatch = read("RULES_DISPATCH")?;
  if !dispatch_on(Some(&dispatch)) {
    return None;
  }
  Some(PassVars {
    excluded: excluded_pigeons(read("RULES_EXCLUDED_PIGEONS").as_deref()),
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
  /// The call rejected or lost the 2 s race. `cpu_limit` when the rejection said so.
  Failed {
    /// See [`is_cpu_limit`].
    cpu_limit: bool,
  },
  /// The script answered. `body` holds at most [`read_cap`] + 1 bytes.
  Answered {
    /// HTTP status.
    status: u16,
    /// The body as read.
    body: Vec<u8>,
  },
}

/// Whether a rejected dispatch's message names the CPU limit. The text is not a contract; a miss
/// records `dispatch_failed`, which counts toward auto-pause all the same.
pub fn is_cpu_limit(message: &str) -> bool {
  message.contains("exceeded CPU")
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
  /// The dispatch hit its CPU limit.
  CpuLimit,
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
      RunStatus::CpuLimit => "cpu_limit",
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
  /// The dispatch failed whole: every rule records this status and the flock is skipped for the
  /// rest of the invocation.
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
    DispatchOutcome::Failed { cpu_limit: true } => return Classified::Flock(RunStatus::CpuLimit),
    DispatchOutcome::Failed { cpu_limit: false } => {
      return Classified::Flock(RunStatus::DispatchFailed);
    }
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
fn without_nul(s: &str) -> std::borrow::Cow<'_, str> {
  if s.contains('\0') {
    s.replace('\0', "\u{fffd}").into()
  } else {
    s.into()
  }
}

/// `s` cut to at most `max` bytes on a character boundary.
fn cut(s: &str, max: usize) -> String {
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
  /// This flock's dispatch already failed this invocation.
  FlockFailed,
  /// [`PASS_BUDGET_MS`] is spent.
  Budget,
}

/// One consumer invocation's second pass: which flocks failed and their accounts, whether the
/// breaker tripped, and when the pass started. Times are milliseconds from any fixed origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassLedger {
  started_ms: u64,
  failed_flocks: Vec<String>,
  failed_accounts: Vec<String>,
  tripped: bool,
}

impl PassLedger {
  /// A pass starting at `started_ms`.
  pub fn new(started_ms: u64) -> Self {
    Self {
      started_ms,
      failed_flocks: Vec::new(),
      failed_accounts: Vec::new(),
      tripped: false,
    }
  }

  /// Whether a batch of `flock_id` may dispatch at `now_ms`.
  pub fn may_dispatch(&self, flock_id: &str, now_ms: u64) -> Result<(), Skip> {
    if self.tripped {
      Err(Skip::Breaker)
    } else if self.failed_flocks.iter().any(|f| f == flock_id) {
      Err(Skip::FlockFailed)
    } else if self.budget_spent(now_ms) {
      Err(Skip::Budget)
    } else {
      Ok(())
    }
  }

  /// Whether [`PASS_BUDGET_MS`] has passed by `now_ms`, checked before a batch's first statement.
  pub fn budget_spent(&self, now_ms: u64) -> bool {
    now_ms.saturating_sub(self.started_ms) >= PASS_BUDGET_MS
  }

  /// Records a dispatch of `flock_id`, owned by `account`. A flock failure skips that flock for
  /// the rest of the pass; failures under a second account, or a platform failure, trip the
  /// breaker, so one tenant cannot trip it alone however many flocks it owns.
  pub fn record(&mut self, flock_id: &str, account: &str, classified: &Classified) {
    match classified {
      Classified::Platform => self.tripped = true,
      Classified::Flock(_) => {
        if !self.failed_flocks.iter().any(|f| f == flock_id) {
          self.failed_flocks.push(flock_id.to_string());
        }
        if !self.failed_accounts.iter().any(|a| a == account) {
          self.failed_accounts.push(account.to_string());
        }
        if self.failed_accounts.len() >= 2 {
          self.tripped = true;
        }
      }
      Classified::PerRule(_) => {}
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

/// Which script a name is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
  /// A flock's live bundle, `flock-<flock uuid>`.
  Flock,
  /// A rule's dry run, `test-<rule uuid>`.
  Test,
}

/// The environment a script is tagged with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesEnv {
  /// `env-prod`.
  Production,
  /// `env-staging`.
  Staging,
}

fn is_uuid(s: &str) -> bool {
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
    ScriptKind::Test => "test-",
  };
  let mut name = String::with_capacity(prefix.len() + 32);
  name.push_str(prefix);
  name.extend(uuid.chars().filter(|c| *c != '-'));
  Some(name)
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
  "// Generated by dovecote: each enabled rule at its deployed version.\n";
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

/// The upload's metadata part. Every field is stated so nothing is inherited: no bindings, no
/// subrequests, CPU per [`ScriptKind`], observability off. `subject` is the flock's uuid for a
/// live bundle and the rule's for a dry run; `account` is the owning org or user.
pub fn upload_metadata(
  kind: ScriptKind,
  account: &str,
  subject: &str,
  env: RulesEnv,
) -> Option<String> {
  if !is_uuid(account) || !is_uuid(subject) {
    return None;
  }
  let (cpu_ms, subject_tag) = match kind {
    ScriptKind::Flock => (RULE_CPU_MS, concat2("flock-", subject)),
    ScriptKind::Test => (TEST_RULE_CPU_MS, "test".to_string()),
  };
  let env_tag = match env {
    RulesEnv::Production => "env-prod",
    RulesEnv::Staging => "env-staging",
  };
  let metadata = serde_json::json!({
    "main_module": "main.js",
    "compatibility_date": RULES_COMPATIBILITY_DATE,
    "bindings": [],
    "limits": { "cpu_ms": cpu_ms, "subrequests": 0 },
    "tags": ["pidgeiot-rule", concat2("acct-", account), subject_tag, env_tag],
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

/// One of the assistant's synthetic test cases, as its structured output gives it.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
  /// Shown beside the case's outcome.
  pub name: String,
  /// Batches in the order they arrive.
  pub events: Vec<CaseEvent>,
  /// The mails the platform should send over the whole case.
  pub expect: Vec<ExpectedMail>,
}

/// One batch of a test case.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaseEvent {
  /// Which of the three abstract pigeons sent it.
  pub pigeon: CasePigeon,
  /// Its readings, in any order.
  pub readings: Vec<CaseReading>,
}

/// The three abstract pigeons a case may name; the model never sees a real id.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CasePigeon {
  /// Pigeon "a".
  A,
  /// Pigeon "b".
  B,
  /// Pigeon "c".
  C,
}

/// One reading of a test case.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaseReading {
  /// Seconds after the case's start.
  pub after_secs: i64,
  /// Key and value pairs, since a schema with a map-shaped object is refused.
  pub metrics: Vec<CaseMetric>,
}

/// One metric of a case reading.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaseMetric {
  /// The telemetry key.
  pub key: String,
  /// Its value, a string like every telemetry value.
  pub value: String,
}

/// A mail a case expects.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExpectedMail {
  /// The alert key.
  pub key: String,
  /// Its kind.
  pub kind: RuleAlertKind,
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
  case: &TestCase,
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
    let mut sorted: Vec<&CaseReading> = event.readings.iter().collect();
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

fn case_pigeon(pigeon: CasePigeon) -> (usize, String, &'static str) {
  match pigeon {
    CasePigeon::A => (0, "a".repeat(64), "Pigeon a"),
    CasePigeon::B => (1, "b".repeat(64), "Pigeon b"),
    CasePigeon::C => (2, "c".repeat(64), "Pigeon c"),
  }
}

fn case_metrics(pairs: &[CaseMetric]) -> Result<BTreeMap<String, String>, CaseError> {
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
      _ => None,
    });
    let vars = vars.expect("on");
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
    let rejected = DispatchOutcome::Failed { cpu_limit: false };
    assert_eq!(
      classify(&one, rejected),
      Classified::Flock(RunStatus::DispatchFailed)
    );
    let cpu = DispatchOutcome::Failed { cpu_limit: true };
    assert_eq!(classify(&one, cpu), Classified::Flock(RunStatus::CpuLimit));
    assert!(is_cpu_limit("Worker exceeded CPU time limit."));
    assert!(!is_cpu_limit("Worker not found"));
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
    ledger.record("f1", "a1", &classified);
    assert_eq!(ledger.may_dispatch("f1", 10), Ok(()));
    assert_eq!(ledger.may_dispatch("f2", 10), Ok(()));

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
    ledger.record("f1", "a1", &failed);
    assert_eq!(ledger.may_dispatch("f1", 1_100), Err(Skip::FlockFailed));
    assert_eq!(ledger.may_dispatch("f2", 1_100), Ok(()));
    ledger.record("f1", "a1", &Classified::Flock(RunStatus::Oversize));
    ledger.record("f2", "a1", &Classified::Flock(RunStatus::CpuLimit));
    assert_eq!(ledger.may_dispatch("f2", 1_100), Err(Skip::FlockFailed));
    assert_eq!(
      ledger.may_dispatch("f3", 1_100),
      Ok(()),
      "one account's flocks cannot trip it alone"
    );
    ledger.record("f3", "a2", &failed);
    assert_eq!(ledger.may_dispatch("f4", 1_100), Err(Skip::Breaker));

    let mut ledger = PassLedger::new(1_000);
    ledger.record("f1", "a1", &Classified::Platform);

    assert_eq!(ledger.may_dispatch("f9", 1_100), Err(Skip::Breaker));

    let ledger = PassLedger::new(1_000);
    assert_eq!(
      ledger.may_dispatch("f1", 1_000 + PASS_BUDGET_MS - 1),
      Ok(())
    );
    assert_eq!(
      ledger.may_dispatch("f1", 1_000 + PASS_BUDGET_MS),
      Err(Skip::Budget)
    );
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
      ["main.js", "prelude.js", "pidgeiot.js", "rules.js", &a, &b]
    );
    assert_eq!(modules[0].1, include_str!("../../rules/main.js"));
    assert_eq!(modules[3].1, manifest);

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

  fn case(json: Value) -> TestCase {
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
      serde_json::from_value::<TestCase>(json!({
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
}
