//! A rule's dry run: the version replayed over the flock's recent telemetry beside the deployed
//! version, in the namespace the live rules run in, with nothing applied.
//!
//! The route uploads `test-<rule>` at [`TEST_RULE_CPU_MS`](super::rules::TEST_RULE_CPU_MS),
//! replays the synthetic cases, the window from empty state and the window seeded with live
//! state, then uploads the same name again at the live limit for the CPU check and deletes it. Two
//! uploads because only the upload's metadata binds CPU. The CPU check is a warning, never a
//! refusal: a fresh script gets about 2 s of grace, so its pass proves little.
//!
//! Every replay answer is read as untrusted, like a live one: re-validated per run, with state
//! threaded here from what dovecote accepts rather than from what the harness threaded.

use std::collections::{BTreeMap, HashMap};

use capsules::{
  DRY_RUN_MAIL_MATCH_SECS, MAX_DRY_RUN_ERRORS, MAX_DRY_RUN_LOGS, MAX_DRY_RUN_MAILS,
  MAX_RULE_ERROR_BYTES, MAX_RULE_LOG_LINE_BYTES, RULE_CONTRACT, RuleAlertScope, RuleCaseMail,
  RuleCaseOutcome, RuleCpuCheck, RuleCpuResult, RuleDryMail, RuleDryRun, RuleErrorGroup,
  RuleMailChange, RuleNamedRef, RuleReplayWindow, RuleReplayed, RuleState, RuleTestCase,
  RuleTestRequest, RuleVersion, merge_state, state_doc_bytes, validate_result,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use tokio_postgres::Client;
use worker::{Env, console_error};

use crate::helpers::cf_api::cf_api_allowed;
use crate::helpers::rule_admin::{BundleSource, Release, dry_run_sources, record_test};
use crate::helpers::rule_dispatch::{DISPATCH_WALL_MS, dispatch};
use crate::helpers::rule_namespace::{
  CfAnswer, Namespace, Refusal, delete_test_script, put_test_script, rules_account,
  test_bundle_keys,
};
use crate::helpers::rule_store::{replay_history, replay_names};
use crate::helpers::rules::{
  AlertFlags, BundleRule, CaseError, Classified, CommonEvent, DispatchOutcome, DispatchRule,
  HARNESS_WALL_MS, MailCounts, RuleMailCounters, RunStatus, ScriptKind, bundle_modules,
  check_case_count, classify, cut, decide_alerts, expand_case, live_input, read_cap,
  regroup_history, script_name, uuid_str, without_nul,
};
use crate::helpers::{FlockAccess, TelemetryHistoryPage};

/// How far back a dry run replays.
pub const REPLAY_WINDOW_SECS: i64 = 7 * 86_400;

/// The race one replay dispatch runs against, the call and the body read together.
pub const REPLAY_WALL_MS: u64 = 30_000;

/// Most bytes of one replay answer read.
pub const REPLAY_READ_CAP: usize = 4 * 1024 * 1024;

/// Batches each side of the CPU check runs: the replay's busiest.
pub const CPU_CHECK_BATCHES: usize = 10;

/// One version's live state as a replay seeds it: each pigeon's document and the flock's.
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct Seed {
  /// Per pigeon id.
  pub pigeons: BTreeMap<String, Map<String, Value>>,
  /// The flock's.
  pub flock: Map<String, Value>,
}

impl Seed {
  /// Whether the rule stores nothing, so a seeded replay would repeat the fresh one.
  pub fn is_empty(&self) -> bool {
    self.flock.is_empty() && self.pigeons.values().all(Map::is_empty)
  }

  /// The two documents a run for `pigeon` is given.
  fn state_for(&self, pigeon: &str) -> RuleState {
    RuleState {
      pigeon: self.pigeons.get(pigeon).cloned().unwrap_or_default(),
      flock: self.flock.clone(),
    }
  }
}

/// One version a replay runs, under its key in the test script.
#[derive(Debug, Clone, Copy)]
pub struct ReplayRule<'a> {
  /// Its key in the script's `rules.js`.
  pub key: &'a str,
  /// Its version.
  pub version: i32,
  /// The live state it starts from; absent, it starts from `{}`.
  pub seed: Option<&'a Seed>,
}

/// `ReplayInput` in `dovecote/rules/wire.d.ts`.
#[derive(Serialize)]
struct ReplayInput<'a> {
  mode: &'static str,
  wall_ms: u32,
  events: &'a [CommonEvent],
  rules: Vec<WireReplayWant<'a>>,
}

#[derive(Serialize)]
struct WireReplayWant<'a> {
  id: &'a str,
  version: i32,
  #[serde(skip_serializing_if = "Option::is_none")]
  seed: Option<&'a Seed>,
}

/// The request body of a replay (`ReplayInput` in `wire.d.ts`).
pub fn replay_input(events: &[CommonEvent], rules: &[ReplayRule]) -> Option<String> {
  let input = ReplayInput {
    mode: "replay",
    wall_ms: HARNESS_WALL_MS,
    events,
    rules: rules
      .iter()
      .map(|r| WireReplayWant {
        id: r.key,
        version: r.version,
        seed: r.seed,
      })
      .collect(),
  };
  serde_json::to_string(&input).ok()
}

/// `ReplayOutput` in `wire.d.ts`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReplayOutput {
  contract: u32,
  #[serde(default)]
  rules: Option<Vec<WireReplayRule>>,
  #[serde(default)]
  error: Option<String>,
}

/// One version's entry of `ReplayOutput.rules`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WireReplayRule {
  id: String,
  status: String,
  #[serde(default)]
  runs: Vec<WireReplayRun>,
  #[serde(default)]
  stopped: Option<WireStopped>,
  #[serde(default, rename = "final")]
  _final: Option<serde::de::IgnoredAny>,
}

/// One listed run; an unlisted index was `ok` with `{}` and no logs.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WireReplayRun {
  i: usize,
  status: String,
  #[serde(default)]
  out_json: Option<String>,
  #[serde(default)]
  error: Option<String>,
  #[serde(default)]
  logs: Vec<String>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
struct WireStopped {
  at: usize,
  status: String,
}

/// Why a replay gave no answer to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayFailure {
  /// The binding could not hand out the script.
  Platform,
  /// The call failed or lost its race: most likely the replay ran past the script's CPU.
  Lost,
  /// Over [`REPLAY_READ_CAP`].
  Oversize,
  /// Not the harness's answer, or the harness failed.
  NotHarness,
  /// The script held other versions: another dry run of the rule replaced it.
  Raced,
}

impl ReplayFailure {
  /// What the person is told the dry run did not finish for.
  pub fn as_str(self) -> &'static str {
    match self {
      ReplayFailure::Platform => "the rules namespace could not run it",
      ReplayFailure::Lost => "the replay was stopped, most likely for CPU or memory",
      ReplayFailure::Oversize => "the replay produced more than 4 MiB of output",
      ReplayFailure::NotHarness => "the replay's answer could not be read",
      ReplayFailure::Raced => "another test of this rule ran at the same time",
    }
  }
}

/// Reads a replay's answer against the versions it named and the events it carried: the
/// harness's own shape, one entry per version in order, each run's index inside the events and
/// increasing, statuses only a replay gives.
pub fn read_replay(
  outcome: DispatchOutcome,
  sent: &[ReplayRule],
  events: usize,
) -> Result<Vec<WireReplayRule>, ReplayFailure> {
  let body = match outcome {
    DispatchOutcome::Binding => return Err(ReplayFailure::Platform),
    DispatchOutcome::Failed => return Err(ReplayFailure::Lost),
    DispatchOutcome::Answered { status: 200, body } => body,
    DispatchOutcome::Answered { .. } => return Err(ReplayFailure::NotHarness),
  };
  if body.len() > REPLAY_READ_CAP {
    return Err(ReplayFailure::Oversize);
  }
  let Ok(output) = serde_json::from_slice::<WireReplayOutput>(&body) else {
    return Err(ReplayFailure::NotHarness);
  };
  let rules = match output {
    WireReplayOutput {
      contract: RULE_CONTRACT,
      rules: Some(rules),
      error: None,
    } if rules.len() == sent.len() => rules,
    _ => return Err(ReplayFailure::NotHarness),
  };
  for (want, got) in sent.iter().zip(&rules) {
    if got.id != want.key {
      return Err(ReplayFailure::NotHarness);
    }
    match got.status.as_str() {
      "ok" => {}
      "version_mismatch" => return Err(ReplayFailure::Raced),
      _ => return Err(ReplayFailure::NotHarness),
    }
    let ordered = got.runs.windows(2).all(|w| w[0].i < w[1].i);
    let known = got.runs.iter().all(|r| {
      r.i < events
        && match r.status.as_str() {
          "ok" => r.out_json.is_some(),
          "threw" | "timeout" | "bad_output" => true,
          _ => false,
        }
    });
    let stopped_ok = got
      .stopped
      .as_ref()
      .is_none_or(|s| s.at < events && s.status == "timeout");
    if !(ordered && known && stopped_ok) {
      return Err(ReplayFailure::NotHarness);
    }
  }
  Ok(rules)
}

fn time_of(secs: i64) -> Option<OffsetDateTime> {
  OffsetDateTime::from_unix_timestamp(secs).ok()
}

/// Error texts in the order first seen, with their counts.
#[derive(Default)]
struct ErrorTally(Vec<RuleErrorGroup>);

impl ErrorTally {
  fn add(&mut self, message: &str) {
    match self.0.iter_mut().find(|g| g.message == message) {
      Some(group) => group.count += 1,
      None => self.0.push(RuleErrorGroup {
        message: message.to_string(),
        count: 1,
      }),
    }
  }

  /// The most frequent first, ties in the order first seen, at most [`MAX_DRY_RUN_ERRORS`].
  fn top(mut self) -> Vec<RuleErrorGroup> {
    self.0.sort_by(|a, b| b.count.cmp(&a.count));
    self.0.truncate(MAX_DRY_RUN_ERRORS);
    self.0
  }
}

/// The mail flags and counters live keeps, threaded through a replay.
#[derive(Default)]
struct MailSim {
  pigeon_flags: HashMap<String, AlertFlags>,
  flock_flags: AlertFlags,
  counters: RuleMailCounters,
}

impl MailSim {
  /// Decides one run's alerts at the batch's last reading, as live would, and moves the flags.
  fn decide(
    &mut self,
    event: &CommonEvent,
    alerts: &[capsules::RuleAlert],
    mails: &mut Vec<RuleDryMail>,
  ) {
    let now = event.received_at;
    let last = event.readings.last().map_or(now, |r| r.at);
    let pigeon = self
      .pigeon_flags
      .entry(event.pigeon.id.clone())
      .or_default();
    let mut flags = self.flock_flags.clone();
    flags.extend(pigeon.iter().map(|(k, v)| (k.clone(), *v)));
    let mut counts = MailCounts::at(self.counters, 0, 0, now);
    for decision in decide_alerts(&flags, alerts, last, now, &mut counts) {
      let key = (decision.alert.scope, decision.alert.key.clone());
      match decision.alert.scope {
        RuleAlertScope::Pigeon => pigeon.insert(key, decision.flag),
        RuleAlertScope::Flock => self.flock_flags.insert(key, decision.flag),
      };
      let Some(at) = time_of(now) else {
        continue;
      };
      mails.push(RuleDryMail {
        at,
        pigeon_id: event.pigeon.id.clone(),
        pigeon_name: event.pigeon.name.clone(),
        key: decision.alert.key,
        kind: decision.alert.kind,
        severity: decision.alert.severity,
        subject: decision.alert.subject,
        sent: decision.mailed,
      });
    }
    self.counters = RuleMailCounters {
      hour_start: Some(now - now.rem_euclid(3_600)),
      in_hour: counts.rule_hour,
      day: Some(now.div_euclid(86_400)),
      in_day: counts.rule_day,
    };
  }
}

/// One version's replay as the page shows it, and every mail it would have sent.
pub fn summarize(
  version: i32,
  events: &[CommonEvent],
  rule: &WireReplayRule,
  seed: Option<&Seed>,
) -> (RuleReplayed, Vec<RuleDryMail>) {
  let mut state = seed.cloned().unwrap_or_default();
  let mut sim = MailSim::default();
  let mut mails = Vec::new();
  let mut errors = ErrorTally::default();
  let mut out = RuleReplayed {
    version,
    ..RuleReplayed::default()
  };
  let ran = rule.stopped.as_ref().map_or(events.len(), |s| s.at + 1);
  let mut listed = rule.runs.iter().peekable();
  for (i, event) in events.iter().enumerate().take(ran) {
    out.runs += 1;
    let run = listed.next_if(|r| r.i == i);
    for line in run.map_or(&[][..], |r| &r.logs[..]) {
      if out.logs.len() < MAX_DRY_RUN_LOGS {
        out
          .logs
          .push(cut(&without_nul(line), MAX_RULE_LOG_LINE_BYTES));
      }
    }
    let error = run
      .and_then(|r| r.error.as_deref())
      .map(|e| cut(&without_nul(e), MAX_RULE_ERROR_BYTES));
    match run.map_or("ok", |r| r.status.as_str()) {
      "ok" => {
        let out_json = run.and_then(|r| r.out_json.as_deref()).unwrap_or("{}");
        let pigeon = event.pigeon.id.as_str();
        match validate_result(out_json, &state.state_for(pigeon)) {
          Ok(result) => {
            out.ok += 1;
            out.values += u32::try_from(result.values.len()).unwrap_or(u32::MAX);
            if let Some(patch) = &result.state.pigeon {
              let doc = state.pigeons.entry(pigeon.to_string()).or_default();
              *doc = merge_state(doc, patch);
            }
            if let Some(patch) = &result.state.flock {
              state.flock = merge_state(&state.flock, patch);
            }
            sim.decide(event, &result.alerts, &mut mails);
          }
          Err(violation) => {
            out.bad_output += 1;
            errors.add(&violation.to_string());
            // The harness threaded this result's state; live would apply none of it.
            let threaded = serde_json::from_str::<Value>(out_json)
              .ok()
              .is_some_and(|v| v.get("state").is_some());
            if threaded && out.diverged_at.is_none() {
              out.diverged_at = time_of(event.received_at);
            }
          }
        }
      }
      "threw" => {
        out.threw += 1;
        errors.add(error.as_deref().unwrap_or("the rule threw"));
      }
      "timeout" => {
        out.timeout += 1;
        errors.add("ran past its time budget");
      }
      _ => {
        out.bad_output += 1;
        errors.add(error.as_deref().unwrap_or("result refused"));
      }
    }
  }
  if let Some(stopped) = &rule.stopped {
    out.stopped_at = events.get(stopped.at).and_then(|e| time_of(e.received_at));
  }
  out.errors = errors.top();
  out.mails_total = u32::try_from(mails.len()).unwrap_or(u32::MAX);
  out.mails = mails.iter().take(MAX_DRY_RUN_MAILS).cloned().collect();
  out.pigeon_state_bytes = state
    .pigeons
    .values()
    .map(state_doc_bytes)
    .max()
    .map_or(0, |b| u32::try_from(b).unwrap_or(u32::MAX));
  out.flock_state_bytes = u32::try_from(state_doc_bytes(&state.flock)).unwrap_or(u32::MAX);
  (out, mails)
}

/// The candidate's mails against the deployed version's: one of each pairs off when pigeon, key
/// and kind agree and they are at most [`DRY_RUN_MAIL_MATCH_SECS`] apart, earliest first.
pub fn diff_mails(candidate: &[RuleDryMail], deployed: &[RuleDryMail]) -> RuleMailChange {
  let mut taken = vec![false; deployed.len()];
  let mut change = RuleMailChange::default();
  for mail in candidate {
    let pair = deployed.iter().enumerate().position(|(j, d)| {
      !taken[j]
        && d.pigeon_id == mail.pigeon_id
        && d.key == mail.key
        && d.kind == mail.kind
        && (d.at - mail.at).whole_seconds().abs() <= DRY_RUN_MAIL_MATCH_SECS
    });
    match pair {
      Some(j) => {
        taken[j] = true;
        change.unchanged += 1;
      }
      None => {
        if change.new.len() < MAX_DRY_RUN_MAILS {
          change.new.push(mail.clone());
        }
      }
    }
  }
  change.gone = deployed
    .iter()
    .zip(&taken)
    .filter(|(_, t)| !**t)
    .map(|(d, _)| d.clone())
    .take(MAX_DRY_RUN_MAILS)
    .collect();
  change
}

/// A case's mails by key and kind.
fn case_mails(mails: &[RuleDryMail]) -> Vec<RuleCaseMail> {
  mails
    .iter()
    .map(|m| RuleCaseMail {
      key: m.key.clone(),
      kind: m.kind,
    })
    .collect()
}

/// Whether two mail lists hold the same mails in any order.
fn same_mails(a: &[RuleCaseMail], b: &[RuleCaseMail]) -> bool {
  let mut left: Vec<&RuleCaseMail> = a.iter().collect();
  b.len() == a.len()
    && b.iter().all(|m| match left.iter().position(|l| *l == m) {
      Some(at) => {
        left.swap_remove(at);
        true
      }
      None => false,
    })
}

/// A case's outcome from its replay. Every decided mail counts, a capped one too: a cap is the
/// platform's, not the rule's.
pub fn case_outcome(
  case: &RuleTestCase,
  replayed: &RuleReplayed,
  mails: &[RuleDryMail],
) -> RuleCaseOutcome {
  let got = case_mails(mails);
  let failed = replayed.threw + replayed.timeout + replayed.bad_output;
  RuleCaseOutcome {
    name: case.name.clone(),
    passed: failed == 0 && same_mails(&case.expect, &got),
    expected: case.expect.clone(),
    got,
    error: replayed.errors.first().map(|g| g.message.clone()),
  }
}

/// One side of the CPU check from its live dispatch: a lost race, a whole failure or a rule out of
/// harness time is over the limit; the platform's own failure judges nothing.
pub fn cpu_verdict(classified: &Classified) -> Option<RuleCpuResult> {
  match classified {
    Classified::Platform => None,
    Classified::Flock(_) => Some(RuleCpuResult::Over),
    Classified::PerRule(runs) => Some(
      if runs
        .iter()
        .any(|r| matches!(r.status, RunStatus::Timeout | RunStatus::Skipped))
      {
        RuleCpuResult::Over
      } else {
        RuleCpuResult::Pass
      },
    ),
  }
}

/// The indexes of the replay's busiest batches, most readings first, ties oldest first.
pub fn busiest(events: &[CommonEvent], n: usize) -> Vec<usize> {
  let mut order: Vec<usize> = (0..events.len()).collect();
  order.sort_by(|&a, &b| events[b].readings.len().cmp(&events[a].readings.len()));
  order.truncate(n);
  order
}

/// What a dry run needs beyond the namespace: the versions, the flock's would-be live bundle, the
/// replay window, the cases and the live state.
pub struct DryRunPlan<'a> {
  /// The rule.
  pub rule: Uuid,
  /// The flock, as the cases' events name it.
  pub flock: RuleNamedRef,
  /// The tested version and its code.
  pub candidate: (i32, &'a str),
  /// The deployed version and its code, when it is another version.
  pub deployed: Option<(i32, &'a str)>,
  /// Every enabled rule of the flock at its deployed version with the candidate in its own place,
  /// in dispatch order: what live would run once this version is deployed.
  pub bundle: &'a [BundleSource],
  /// The replay's batches, oldest last reading first.
  pub events: &'a [CommonEvent],
  /// What the events cover.
  pub window: RuleReplayWindow,
  /// The synthetic cases, already counted.
  pub cases: &'a [RuleTestCase],
  /// Each bundle rule's live state; the candidate's seeds its second replay.
  pub seeds: &'a HashMap<Uuid, Seed>,
  /// Unix seconds the cases start at.
  pub now: i64,
}

/// Why a dry run gave no result. Nothing was recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum DryRunRefusal {
  /// The platform's API budget is spent: nothing was uploaded.
  Busy(u32),
  /// The test script's upload was refused or not confirmed.
  Upload(CfAnswer),
  /// A replay did not finish.
  Unfinished(ReplayFailure),
  /// A case does not expand; refused before anything was uploaded.
  Case(CaseError),
  /// The script could not be built.
  Unbuildable,
}

/// Everything a dry run does outside itself, so each step is a host test.
pub(crate) trait DryRunIo {
  /// Spends one call of the platform's Cloudflare API budget; false when it is spent.
  async fn allowed(&mut self) -> bool;
  /// Uploads `test-<rule>` as `kind` holding `modules`.
  async fn upload(
    &mut self,
    kind: ScriptKind,
    modules: &[(String, String)],
  ) -> Result<(), CfAnswer>;
  /// One dispatch of `test-<rule>`, its body read to at most `cap` + 1 bytes within `wall_ms`.
  async fn dispatch(&mut self, body: &str, cap: usize, wall_ms: u64) -> DispatchOutcome;
  /// Deletes `test-<rule>`; a failure is left to the daily sweep.
  async fn delete(&mut self);
}

/// The wait told after the platform's own limiter refuses.
const BUSY_SECS: u32 = crate::helpers::cf_api::CF_API_WINDOW_SECS;

/// Runs the whole dry run, deleting the test script whenever an upload may have landed.
pub(crate) async fn run_dry_run<I: DryRunIo>(
  io: &mut I,
  plan: &DryRunPlan<'_>,
) -> Result<RuleDryRun, DryRunRefusal> {
  if let Err(problem) = check_cases(plan) {
    return Err(DryRunRefusal::Case(problem));
  }
  let rule = uuid_str(plan.rule);
  let Some((key, deployed_key)) =
    test_bundle_keys(&rule, plan.candidate.0, plan.deployed.map(|d| d.0))
  else {
    return Err(DryRunRefusal::Unbuildable);
  };
  let Some(modules) =
    crate::helpers::rule_namespace::test_script_modules(plan.rule, plan.candidate, plan.deployed)
  else {
    return Err(DryRunRefusal::Unbuildable);
  };
  if !io.allowed().await {
    return Err(DryRunRefusal::Busy(BUSY_SECS));
  }
  if let Err(answer) = io.upload(ScriptKind::Test, &modules).await {
    if answer.may_have_landed() {
      io.delete().await;
    }
    return Err(DryRunRefusal::Upload(answer));
  }
  let result = replay_all(io, plan, &key, deployed_key.as_deref()).await;
  let result = match result {
    Ok(mut run) => {
      run.cpu = cpu_check(io, plan).await;
      Ok(run)
    }
    Err(failure) => Err(DryRunRefusal::Unfinished(failure)),
  };
  io.delete().await;
  result
}

fn check_cases(plan: &DryRunPlan) -> Result<(), CaseError> {
  if plan.cases.is_empty() {
    return Ok(());
  }
  check_case_count(plan.cases.len())?;
  for case in plan.cases {
    expand_case(case, &plan.flock, plan.now)?;
  }
  Ok(())
}

/// One replay dispatch, read.
async fn replay<I: DryRunIo>(
  io: &mut I,
  events: &[CommonEvent],
  rules: &[ReplayRule<'_>],
) -> Result<Vec<WireReplayRule>, ReplayFailure> {
  let Some(body) = replay_input(events, rules) else {
    return Err(ReplayFailure::NotHarness);
  };
  let outcome = io.dispatch(&body, REPLAY_READ_CAP, REPLAY_WALL_MS).await;
  read_replay(outcome, rules, events.len())
}

/// The cases, the window from empty state beside the deployed version, then the window seeded.
async fn replay_all<I: DryRunIo>(
  io: &mut I,
  plan: &DryRunPlan<'_>,
  key: &str,
  deployed_key: Option<&str>,
) -> Result<RuleDryRun, ReplayFailure> {
  let (version, _) = plan.candidate;
  let fresh = ReplayRule {
    key,
    version,
    seed: None,
  };

  let mut cases = Vec::with_capacity(plan.cases.len());
  for case in plan.cases {
    let Ok(events) = expand_case(case, &plan.flock, plan.now) else {
      return Err(ReplayFailure::NotHarness);
    };
    let answer = replay(io, &events, &[fresh]).await?;
    let (replayed, mails) = summarize(version, &events, &answer[0], None);
    cases.push(case_outcome(case, &replayed, &mails));
  }

  let deployed = plan.deployed.zip(deployed_key);
  let (candidate, deployed_run, mails) = if plan.events.is_empty() {
    let empty = |version| RuleReplayed {
      version,
      ..RuleReplayed::default()
    };
    let deployed_run = deployed.map(|((v, _), _)| empty(v));
    (empty(version), deployed_run, RuleMailChange::default())
  } else {
    let mut wants = vec![fresh];
    if let Some(((v, _), dkey)) = deployed {
      wants.push(ReplayRule {
        key: dkey,
        version: v,
        seed: None,
      });
    }
    let answer = replay(io, plan.events, &wants).await?;
    let (candidate, new_mails) = summarize(version, plan.events, &answer[0], None);
    match (deployed, answer.get(1)) {
      (Some(((v, _), _)), Some(entry)) => {
        let (deployed_run, old_mails) = summarize(v, plan.events, entry, None);
        (
          candidate,
          Some(deployed_run),
          diff_mails(&new_mails, &old_mails),
        )
      }
      _ => (candidate, None, diff_mails(&new_mails, &[])),
    }
  };

  let live = plan.seeds.get(&plan.rule).filter(|s| !s.is_empty());
  let seeded = match live {
    Some(seed) if !plan.events.is_empty() => {
      let want = ReplayRule {
        seed: Some(seed),
        ..fresh
      };
      let answer = replay(io, plan.events, &[want]).await?;
      Some(summarize(version, plan.events, &answer[0], Some(seed)).0)
    }
    _ => None,
  };

  Ok(RuleDryRun {
    window: plan.window.clone(),
    candidate,
    deployed: deployed_run,
    seeded,
    mails,
    cases,
    cpu: RuleCpuCheck::default(),
  })
}

/// The CPU check: the would-be bundle uploaded at the live limit under the test script's name,
/// then the candidate alone and the bundle as live dispatches over the busiest batches, each side
/// stopping at its first batch over the limit. Never a refusal.
async fn cpu_check<I: DryRunIo>(io: &mut I, plan: &DryRunPlan<'_>) -> RuleCpuCheck {
  let mut check = RuleCpuCheck::default();
  if plan.events.is_empty() || !io.allowed().await {
    return check;
  }
  let keys: Vec<String> = plan.bundle.iter().map(|s| uuid_str(s.rule_id)).collect();
  let rules: Vec<BundleRule> = plan
    .bundle
    .iter()
    .zip(&keys)
    .map(|(s, key)| BundleRule {
      key,
      version: s.version,
      source: &s.source,
    })
    .collect();
  let Some(modules) = bundle_modules(&rules) else {
    return check;
  };
  match io.upload(ScriptKind::CpuCheck, &modules).await {
    Ok(()) => {}
    Err(CfAnswer::Invalid(message)) => {
      check.candidate = RuleCpuResult::DidNotLoad;
      check.bundle = RuleCpuResult::DidNotLoad;
      check.load_error = Some(message);
      return check;
    }
    Err(_) => return check,
  }
  let dispatch_rule = |source: &BundleSource, key: &str, pigeon: &str| DispatchRule {
    id: key.to_string(),
    version: source.version,
    state: plan
      .seeds
      .get(&source.rule_id)
      .map(|s| s.state_for(pigeon))
      .unwrap_or_default(),
  };
  let batches = busiest(plan.events, CPU_CHECK_BATCHES);
  check.batches = u32::try_from(batches.len()).unwrap_or(u32::MAX);
  let alone_only = plan.bundle.len() <= 1;
  for alone in [true, false] {
    if !alone && alone_only {
      check.bundle = check.candidate;
      break;
    }
    let mut side = RuleCpuResult::Pass;
    for &i in &batches {
      let event = &plan.events[i];
      let sent: Vec<DispatchRule> = plan
        .bundle
        .iter()
        .zip(&keys)
        .filter(|(s, _)| !alone || s.rule_id == plan.rule)
        .map(|(s, key)| dispatch_rule(s, key, &event.pigeon.id))
        .collect();
      let Some(body) = live_input(event, &sent) else {
        side = RuleCpuResult::NotRun;
        break;
      };
      let outcome = io
        .dispatch(&body, read_cap(sent.len()), DISPATCH_WALL_MS)
        .await;
      match cpu_verdict(&classify(&sent, outcome)) {
        Some(RuleCpuResult::Pass) => {}
        Some(other) => {
          side = other;
          break;
        }
        None => {
          side = RuleCpuResult::NotRun;
          break;
        }
      }
    }
    if alone {
      check.candidate = side;
    } else {
      check.bundle = side;
    }
  }
  check
}

/// The dry run against this Worker's own namespace.
struct LiveDryRun<'a> {
  env: &'a Env,
  ns: &'a Namespace,
  rule: Uuid,
  account: Uuid,
  script: String,
  flock_id: &'a str,
}

impl DryRunIo for LiveDryRun<'_> {
  async fn allowed(&mut self) -> bool {
    cf_api_allowed(self.env).await
  }

  async fn upload(
    &mut self,
    kind: ScriptKind,
    modules: &[(String, String)],
  ) -> Result<(), CfAnswer> {
    put_test_script(self.env, self.ns, self.rule, self.account, kind, modules).await
  }

  async fn dispatch(&mut self, body: &str, cap: usize, wall_ms: u64) -> DispatchOutcome {
    dispatch(self.env, &self.script, self.flock_id, body, cap, wall_ms).await
  }

  async fn delete(&mut self) {
    // The cleanup spends a call whatever the limiter says, so nothing is orphaned.
    let _ = cf_api_allowed(self.env).await;
    if delete_test_script(self.env, self.ns, self.rule)
      .await
      .is_err()
    {
      console_error!("rules: a dry run's script was not deleted; the sweep will");
    }
  }
}

/// The window `events` cover, and whether readings inside it were left out: by the history
/// read's own cap, or by [`REPLAY_MAX_READINGS`](super::rules::REPLAY_MAX_READINGS).
fn replay_window(events: &[CommonEvent], points: &TelemetryHistoryPage) -> RuleReplayWindow {
  let readings: usize = events.iter().map(|e| e.readings.len()).sum();
  let mut pigeons: Vec<&str> = events.iter().map(|e| e.pigeon.id.as_str()).collect();
  pigeons.sort_unstable();
  pigeons.dedup();
  let read: std::collections::BTreeSet<(&str, i64)> = points
    .points
    .iter()
    .map(|p| (p.pigeon_id.as_str(), p.reported_at.unix_timestamp()))
    .collect();
  let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
  RuleReplayWindow {
    from: events
      .iter()
      .filter_map(|e| e.readings.first())
      .map(|r| r.at)
      .min()
      .and_then(time_of),
    to: events.iter().map(|e| e.received_at).max().and_then(time_of),
    readings: count(readings),
    batches: count(events.len()),
    pigeons: count(pigeons.len()),
    cut: points.truncated || read.len() > readings,
  }
}

const VERSION_DID_NOT_LOAD: &str = "Unprocessable Content: this version did not load: ";
const DID_NOT_FINISH: &str = "Unprocessable Content: the dry run did not finish: ";

/// The route's answer to a dry run that gave no result.
fn refusal_of(refusal: DryRunRefusal) -> Refusal {
  match refusal {
    DryRunRefusal::Busy(wait) => Refusal::Busy(wait),
    DryRunRefusal::Upload(CfAnswer::Invalid(message)) => {
      let mut text = String::with_capacity(VERSION_DID_NOT_LOAD.len() + message.len());
      text.push_str(VERSION_DID_NOT_LOAD);
      text.push_str(&message);
      Refusal::Status(422, text)
    }
    DryRunRefusal::Upload(answer) => Refusal::of(&answer),
    DryRunRefusal::Unfinished(ReplayFailure::Raced) => Refusal::Status(
      409,
      "Conflict: another test of this rule ran at the same time; test again".to_string(),
    ),
    DryRunRefusal::Unfinished(ReplayFailure::Platform) => Refusal::of(&CfAnswer::Refused {
      status: 0,
      codes: Vec::new(),
    }),
    DryRunRefusal::Unfinished(failure) => {
      let why = failure.as_str();
      let mut text = String::with_capacity(DID_NOT_FINISH.len() + why.len());
      text.push_str(DID_NOT_FINISH);
      text.push_str(why);
      Refusal::Status(422, text)
    }
    DryRunRefusal::Case(problem) => {
      let why = problem.as_str();
      let mut text = String::with_capacity(13 + why.len());
      text.push_str("Bad Request: ");
      text.push_str(why);
      Refusal::Status(400, text)
    }
    DryRunRefusal::Unbuildable => Refusal::internal(),
  }
}

/// The test route once its own refusals are passed: reads the flock's last
/// [`REPLAY_WINDOW_SECS`] of telemetry and what the bundle holds, runs the dry run in the
/// namespace, and records it on the version, answering the version as the single read does.
pub async fn test_version(
  env: &Env,
  ns: &Namespace,
  client: &Client,
  access: &FlockAccess,
  release: &Release,
  request: &RuleTestRequest,
) -> Result<RuleVersion, Refusal> {
  let (_, account) = rules_account(access)?;
  let rule_id = release.rule.id;
  let version = release.version.version;
  let excluded = env
    .var("RULES_EXCLUDED_PIGEONS")
    .ok()
    .map(|v| v.to_string());
  let now = OffsetDateTime::now_utc();
  let since = now - time::Duration::seconds(REPLAY_WINDOW_SECS);
  let history = replay_history(client, access, since, excluded.as_deref())
    .await
    .map_err(|_| Refusal::internal())?;
  let names = replay_names(client, access, excluded.as_deref())
    .await
    .map_err(|_| Refusal::internal())?;
  let Some((flock_name, names)) = names else {
    return Err(Refusal::NotFound);
  };
  let flock = RuleNamedRef {
    id: access.flock_id().to_string(),
    name: flock_name,
  };
  let events = regroup_history(&history.points, &flock, &names);
  let mut pigeons: Vec<String> = events.iter().map(|e| e.pigeon.id.clone()).collect();
  pigeons.sort_unstable();
  pigeons.dedup();
  let Some(sources) = dry_run_sources(client, access, release, &pigeons)
    .await
    .map_err(|_| Refusal::internal())?
  else {
    return Err(Refusal::NotFound);
  };
  let plan = DryRunPlan {
    rule: rule_id,
    flock,
    candidate: (version, &sources.candidate),
    deployed: sources.deployed.as_ref().map(|(v, s)| (*v, s.as_str())),
    bundle: &sources.bundle,
    window: replay_window(&events, &history),
    events: &events,
    cases: &request.cases,
    seeds: &sources.seeds,
    now: now.unix_timestamp(),
  };
  let Some(script) = script_name(ScriptKind::Test, &uuid_str(rule_id)) else {
    return Err(Refusal::internal());
  };
  let mut io = LiveDryRun {
    env,
    ns,
    rule: rule_id,
    account,
    script,
    flock_id: access.flock_id(),
  };
  let result = run_dry_run(&mut io, &plan).await.map_err(refusal_of)?;
  let Ok(summary) = serde_json::to_string(&result) else {
    return Err(Refusal::internal());
  };
  match record_test(
    client,
    access,
    rule_id,
    version,
    &release.version.sha256,
    &summary,
  )
  .await
  {
    Ok(Some(tested)) => Ok(tested),
    Ok(None) => Err(Refusal::NotFound),
    Err(_) => Err(Refusal::internal()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::helpers::rules::{RunRecord, regroup_history};
  use capsules::{
    RuleAlertKind, RuleCaseEvent, RuleCaseMetric, RuleCasePigeon, RuleCaseReading, RuleHook,
    RuleReading, RuleSeverity, TelemetryHistoryPoint,
  };
  use serde_json::json;
  use std::collections::VecDeque;

  const REPLAY_WORKED: &str = include_str!("../../rules/canned/replay-worked.json");
  const T0: i64 = 1_793_380_000;

  fn flock() -> RuleNamedRef {
    RuleNamedRef {
      id: "f1".into(),
      name: "Bench signs".into(),
    }
  }

  fn ev(pigeon: &str, at: i64, metrics: &[(&str, &str)]) -> CommonEvent {
    CommonEvent {
      contract: RULE_CONTRACT,
      hook: RuleHook::Telemetry,
      batch_id: "b".into(),
      received_at: at,
      degraded: false,
      flock: flock(),
      pigeon: RuleNamedRef {
        id: pigeon.into(),
        name: ["Sign ", pigeon].concat(),
      },
      readings: vec![RuleReading {
        at,
        metrics: metrics
          .iter()
          .map(|(k, v)| (k.to_string(), v.to_string()))
          .collect(),
        previous: BTreeMap::new(),
      }],
    }
  }

  /// The events make-canned.mjs replays for replay-worked.json.
  fn worked_events() -> Vec<CommonEvent> {
    vec![
      ev("a", T0, &[("temp_c", "35")]),
      ev("a", T0 + 60, &[("temp_c", "45")]),
      ev("a", T0 + 120, &[("temp_c", "46")]),
      ev("b", T0 + 130, &[("temp_c", "50")]),
      ev("a", T0 + 180, &[("temp_c", "30")]),
      ev("a", T0 + 240, &[("reset_cause", "1")]),
    ]
  }

  fn want(key: &str) -> ReplayRule<'_> {
    ReplayRule {
      key,
      version: 1,
      seed: None,
    }
  }

  fn answered(body: &str) -> DispatchOutcome {
    DispatchOutcome::Answered {
      status: 200,
      body: body.as_bytes().to_vec(),
    }
  }

  fn replay_body(rules: Value) -> String {
    json!({ "contract": 1, "rules": rules }).to_string()
  }

  #[test]
  fn a_replay_input_is_the_wire_shape_the_harness_reads() {
    let events = worked_events();
    let seed = Seed {
      pigeons: [(
        "a".to_string(),
        json!({ "n": 1 }).as_object().cloned().unwrap(),
      )]
      .into(),
      flock: Map::new(),
    };
    let body = replay_input(
      &events[..1],
      &[
        want("r-v2"),
        ReplayRule {
          key: "r-v1",
          version: 1,
          seed: Some(&seed),
        },
      ],
    )
    .expect("serializes");
    let input: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(input["mode"], "replay");
    assert_eq!(input["wall_ms"], HARNESS_WALL_MS);
    assert_eq!(input["events"][0]["pigeon"]["id"], "a");
    assert!(input["events"][0].get("state").is_none());
    assert_eq!(input["rules"][0], json!({ "id": "r-v2", "version": 1 }));
    assert_eq!(
      input["rules"][1]["seed"],
      json!({ "pigeons": { "a": { "n": 1 } }, "flock": {} })
    );
  }

  #[test]
  fn the_canned_workerd_replay_reads_and_mails_as_live_would() {
    let events = worked_events();
    let sent = [want("starter"), want("r7")];
    let rules = read_replay(answered(REPLAY_WORKED), &sent, events.len()).expect("harness shape");
    let (starter, mails) = summarize(1, &events, &rules[0], None);
    assert_eq!((starter.runs, starter.ok), (6, 6));
    let kinds: Vec<(&str, &str, RuleAlertKind)> = mails
      .iter()
      .map(|m| (m.pigeon_id.as_str(), m.key.as_str(), m.kind))
      .collect();
    assert_eq!(
      kinds,
      [
        ("a", "too-hot", RuleAlertKind::Fire),
        ("b", "too-hot", RuleAlertKind::Fire),
        ("a", "too-hot", RuleAlertKind::Clear),
      ],
      "a restated fire and the first batch's clear with no flag mail nothing"
    );
    assert_eq!(mails[0].at.unix_timestamp(), T0 + 60);
    assert!(mails.iter().all(|m| m.sent));
    let (r7, r7_mails) = summarize(1, &events, &rules[1], None);
    assert_eq!(r7.runs, 6);
    assert!(r7_mails.is_empty());
    assert_eq!(r7.values, 1, "one boot counted");
    assert!(r7.pigeon_state_bytes > 0);
  }

  #[test]
  fn a_replay_answer_not_in_the_harness_shape_is_refused() {
    let events = worked_events();
    let sent = [want("x")];
    let read = |body: Value| read_replay(answered(&body.to_string()), &sent, events.len());
    let entry = |runs: Value| {
      replay_body(json!([{ "id": "x", "status": "ok", "runs": runs,
      "stopped": null, "final": {} }]))
    };
    assert!(read_replay(answered(&entry(json!([]))), &sent, events.len()).is_ok());
    for bad in [
      json!({ "contract": 1, "rules": [] }),
      json!({ "contract": 2, "rules": [{ "id": "x", "status": "ok", "runs": [] }] }),
      json!({ "contract": 1, "error": "harness_failed" }),
      json!({ "contract": 1, "rules": [{ "id": "y", "status": "ok", "runs": [] }] }),
      json!({ "contract": 1, "rules": [{ "id": "x", "status": "ok", "runs": [], "extra": 1 }] }),
    ] {
      assert_eq!(read(bad.clone()), Err(ReplayFailure::NotHarness), "{bad}");
    }
    for runs in [
      json!([{ "i": 9, "status": "threw", "logs": [] }]),
      json!([{ "i": 1, "status": "threw", "logs": [] }, { "i": 0, "status": "threw", "logs": [] }]),
      json!([{ "i": 0, "status": "ok", "logs": [] }]),
      json!([{ "i": 0, "status": "skipped", "logs": [] }]),
    ] {
      let body = entry(runs.clone());
      assert_eq!(
        read_replay(answered(&body), &sent, events.len()),
        Err(ReplayFailure::NotHarness),
        "{runs}"
      );
    }
    let raced = json!({ "contract": 1, "rules": [{ "id": "x", "status": "version_mismatch" }] });
    assert_eq!(read(raced), Err(ReplayFailure::Raced));
    assert_eq!(
      read_replay(DispatchOutcome::Failed, &sent, 1),
      Err(ReplayFailure::Lost)
    );
    assert_eq!(
      read_replay(DispatchOutcome::Binding, &sent, 1),
      Err(ReplayFailure::Platform)
    );
    let huge = vec![b' '; REPLAY_READ_CAP + 1];
    let outcome = DispatchOutcome::Answered {
      status: 200,
      body: huge,
    };
    assert_eq!(read_replay(outcome, &sent, 1), Err(ReplayFailure::Oversize));
  }

  fn entry(runs: Value, stopped: Value) -> WireReplayRule {
    serde_json::from_value(
      json!({ "id": "x", "status": "ok", "runs": runs, "stopped": stopped,
      "final": {} }),
    )
    .expect("entry")
  }

  #[test]
  fn state_is_threaded_from_what_dovecote_accepts_and_a_refusal_marks_divergence() {
    let events: Vec<CommonEvent> = (0..4)
      .map(|n| ev("a", T0 + n * 60, &[("k", "1")]))
      .collect();
    let big = "x".repeat(capsules::MAX_RULE_PIGEON_STATE_BYTES);
    let runs = json!([
      { "i": 0, "status": "ok", "out_json": json!({ "state": { "pigeon": { "n": 1 } } })
        .to_string(), "logs": ["one"] },
      { "i": 1, "status": "ok", "out_json": json!({ "state": { "pigeon": { "big": big } } })
        .to_string(), "logs": [] },
      { "i": 2, "status": "threw", "error": "TypeError: x\u{0}", "logs": [] },
      { "i": 3, "status": "threw", "error": "TypeError: x\u{0}", "logs": [] },
    ]);
    let (out, mails) = summarize(2, &events, &entry(runs, Value::Null), None);
    assert!(mails.is_empty());
    assert_eq!((out.runs, out.ok, out.bad_output, out.threw), (4, 1, 1, 2));
    assert_eq!(out.diverged_at.map(|t| t.unix_timestamp()), Some(T0 + 60));
    assert_eq!(out.errors[0].count, 2, "the most frequent first");
    assert_eq!(
      out.errors[0].message, "TypeError: x\u{fffd}",
      "Postgres refuses U+0000"
    );
    assert_eq!(out.errors[1].message, "state.pigeon: too large");
    assert_eq!(out.logs, ["one"]);
    assert_eq!(
      out.pigeon_state_bytes as usize,
      state_doc_bytes(json!({ "n": 1 }).as_object().unwrap()),
      "only the accepted patch is stored"
    );
  }

  #[test]
  fn a_timeout_stops_the_replay_where_it_happened() {
    let events: Vec<CommonEvent> = (0..5)
      .map(|n| ev("a", T0 + n * 60, &[("k", "1")]))
      .collect();
    let runs = json!([{ "i": 2, "status": "timeout", "logs": [] }]);
    let (out, _) = summarize(
      1,
      &events,
      &entry(runs, json!({ "at": 2, "status": "timeout" })),
      None,
    );
    assert_eq!((out.runs, out.ok, out.timeout), (3, 2, 1));
    assert_eq!(out.stopped_at.map(|t| t.unix_timestamp()), Some(T0 + 120));
  }

  #[test]
  fn a_seed_is_the_state_the_first_run_is_validated_against() {
    let events = vec![ev("a", T0, &[("k", "1")])];
    let big = "x".repeat(capsules::MAX_RULE_PIGEON_STATE_BYTES);
    let seed = Seed {
      pigeons: [(
        "a".to_string(),
        json!({ "big": big }).as_object().cloned().unwrap(),
      )]
      .into(),
      flock: Map::new(),
    };
    let grow = json!({ "state": { "pigeon": { "more": 1 } } }).to_string();
    let runs = json!([{ "i": 0, "status": "ok", "out_json": grow, "logs": [] }]);
    let (fresh, _) = summarize(1, &events, &entry(runs.clone(), Value::Null), None);
    let (seeded, _) = summarize(1, &events, &entry(runs, Value::Null), Some(&seed));
    assert_eq!((fresh.ok, seeded.bad_output), (1, 1));
  }

  fn mail(at: i64, pigeon: &str, key: &str, kind: RuleAlertKind) -> RuleDryMail {
    RuleDryMail {
      at: time_of(at).unwrap(),
      pigeon_id: pigeon.into(),
      pigeon_name: String::new(),
      key: key.into(),
      kind,
      severity: RuleSeverity::Info,
      subject: "s".into(),
      sent: true,
    }
  }

  #[test]
  fn mails_pair_off_by_pigeon_key_and_kind_within_five_minutes() {
    use RuleAlertKind::*;
    let candidate = [
      mail(T0, "a", "hot", Fire),
      mail(T0 + 600, "a", "hot", Clear),
      mail(T0 + 700, "b", "hot", Fire),
    ];
    let deployed = [
      mail(T0 + 300, "a", "hot", Fire),
      mail(T0 + 601 + DRY_RUN_MAIL_MATCH_SECS, "a", "hot", Clear),
      mail(T0 + 700, "a", "hot", Fire),
    ];
    let change = diff_mails(&candidate, &deployed);
    assert_eq!(change.unchanged, 1);
    assert_eq!(change.new.len(), 2);
    assert_eq!(change.gone.len(), 2);
    assert_eq!(
      change.new[1].pigeon_id, "b",
      "another pigeon is another mail"
    );
    let alone = diff_mails(&candidate, &[]);
    assert_eq!((alone.new.len(), alone.gone.len()), (3, 0));
  }

  #[test]
  fn the_hourly_mail_cap_holds_mail_back_as_live_would() {
    let events: Vec<CommonEvent> = (0..12)
      .map(|n| {
        ev(
          if n % 2 == 0 { "a" } else { "b" },
          T0 + n * 60,
          &[("k", "1")],
        )
      })
      .collect();
    let fire = |key: &str| {
      json!({ "alerts": [{ "key": key, "kind": "notify", "severity": "info", "scope": "pigeon",
        "subject": "s" }] })
      .to_string()
    };
    let runs: Vec<Value> = (0..12)
      .map(|i| {
        json!({ "i": i, "status": "ok", "out_json": fire(&["k", &i.to_string()].concat()),
        "logs": [] })
      })
      .collect();
    let (out, mails) = summarize(1, &events, &entry(Value::Array(runs), Value::Null), None);
    assert_eq!(out.mails_total, 12);
    let sent = mails.iter().filter(|m| m.sent).count();
    assert_eq!(
      sent,
      crate::helpers::rules::RULE_MAIL_PER_HOUR as usize,
      "the hourly cap"
    );
  }

  fn case(expect: &[(&str, RuleAlertKind)]) -> RuleTestCase {
    RuleTestCase {
      name: "over".into(),
      events: vec![RuleCaseEvent {
        pigeon: RuleCasePigeon::A,
        readings: vec![RuleCaseReading {
          after_secs: 0,
          metrics: vec![RuleCaseMetric {
            key: "temp_c".into(),
            value: "41".into(),
          }],
        }],
      }],
      expect: expect
        .iter()
        .map(|(k, kind)| RuleCaseMail {
          key: k.to_string(),
          kind: *kind,
        })
        .collect(),
    }
  }

  #[test]
  fn a_case_passes_on_the_same_mails_in_any_order_and_no_failed_run() {
    use RuleAlertKind::*;
    let mails = [mail(T0, "a", "x", Clear), mail(T0, "a", "y", Fire)];
    let ok = RuleReplayed::default();
    let pass = case_outcome(&case(&[("y", Fire), ("x", Clear)]), &ok, &mails);
    assert!(pass.passed);
    let miss = case_outcome(&case(&[("y", Fire)]), &ok, &mails);
    assert!(!miss.passed);
    let threw = RuleReplayed {
      threw: 1,
      errors: vec![RuleErrorGroup {
        message: "boom".into(),
        count: 1,
      }],
      ..RuleReplayed::default()
    };
    let failed = case_outcome(&case(&[("y", Fire), ("x", Clear)]), &threw, &mails);
    assert!(!failed.passed);
    assert_eq!(failed.error.as_deref(), Some("boom"));
  }

  #[test]
  fn the_cpu_verdict_reads_a_kill_as_over_and_the_platform_as_no_answer() {
    let run = |status| RunRecord {
      id: "r".into(),
      status,
      result: None,
      error: None,
      logs: vec![],
    };
    assert_eq!(
      cpu_verdict(&Classified::PerRule(vec![
        run(RunStatus::Ok),
        run(RunStatus::Threw)
      ])),
      Some(RuleCpuResult::Pass),
      "a throw is the rule's bug, not CPU"
    );
    assert_eq!(
      cpu_verdict(&Classified::PerRule(vec![
        run(RunStatus::Ok),
        run(RunStatus::Skipped)
      ])),
      Some(RuleCpuResult::Over)
    );
    assert_eq!(
      cpu_verdict(&Classified::Flock(RunStatus::DispatchFailed)),
      Some(RuleCpuResult::Over)
    );
    assert_eq!(cpu_verdict(&Classified::Platform), None);
  }

  #[test]
  fn the_busiest_batches_come_first_and_ties_keep_their_order() {
    let mut events = worked_events();
    let extra = events[2].readings[0].clone();
    events[2].readings.push(extra);
    assert_eq!(busiest(&events, 3), [2, 0, 1]);
    assert_eq!(busiest(&events, 10).len(), events.len());
  }

  /// A namespace that answers from a script of outcomes.
  #[derive(Default)]
  struct Fake {
    budget: usize,
    upload_answers: VecDeque<Result<(), CfAnswer>>,
    outcomes: VecDeque<DispatchOutcome>,
    calls: Vec<String>,
    bodies: Vec<Value>,
  }

  impl DryRunIo for Fake {
    async fn allowed(&mut self) -> bool {
      self.calls.push("allowed".into());
      let ok = self.budget > 0;
      self.budget = self.budget.saturating_sub(1);
      ok
    }

    async fn upload(
      &mut self,
      kind: ScriptKind,
      modules: &[(String, String)],
    ) -> Result<(), CfAnswer> {
      let names: Vec<&str> = modules.iter().map(|(n, _)| n.as_str()).collect();
      self
        .calls
        .push(format!("upload {kind:?} {}", names.join(" ")));
      self.upload_answers.pop_front().unwrap_or(Ok(()))
    }

    async fn dispatch(&mut self, body: &str, cap: usize, wall_ms: u64) -> DispatchOutcome {
      let request: Value = serde_json::from_str(body).expect("json");
      self
        .calls
        .push(format!("dispatch {} {cap} {wall_ms}", request["mode"]));
      self.bodies.push(request.clone());
      if let Some(outcome) = self.outcomes.pop_front() {
        return outcome;
      }
      // The harness answering every listed rule quietly.
      if request["mode"] == "replay" {
        let rules: Vec<Value> = request["rules"]
          .as_array()
          .expect("rules")
          .iter()
          .map(|r| {
            json!({ "id": r["id"], "status": "ok", "runs": [], "stopped": null,
            "final": {} })
          })
          .collect();
        answered(&replay_body(Value::Array(rules)))
      } else {
        let results: Vec<Value> = request["rules"]
          .as_array()
          .expect("rules")
          .iter()
          .map(|r| json!({ "id": r["id"], "status": "ok", "out_json": "{}", "logs": [] }))
          .collect();
        answered(&json!({ "contract": 1, "results": results }).to_string())
      }
    }

    async fn delete(&mut self) {
      self.calls.push("delete".into());
    }
  }

  const RULE: u128 = 0xa1;
  const SIBLING: u128 = 0xb2;

  struct Inputs {
    bundle: Vec<BundleSource>,
    events: Vec<CommonEvent>,
    seeds: HashMap<Uuid, Seed>,
    cases: Vec<RuleTestCase>,
  }

  fn inputs(siblings: bool) -> Inputs {
    let rule = |id, version, source: &str| BundleSource {
      rule_id: Uuid::from_u128(id),
      version,
      source: source.into(),
    };
    let mut bundle = vec![rule(RULE, 3, "new")];
    if siblings {
      bundle.insert(0, rule(SIBLING, 1, "other"));
    }
    Inputs {
      bundle,
      events: worked_events(),
      seeds: HashMap::new(),
      cases: vec![],
    }
  }

  fn plan(i: &Inputs) -> DryRunPlan<'_> {
    DryRunPlan {
      rule: Uuid::from_u128(RULE),
      flock: flock(),
      candidate: (3, "new"),
      deployed: Some((2, "old")),
      bundle: &i.bundle,
      events: &i.events,
      window: RuleReplayWindow::default(),
      cases: &i.cases,
      seeds: &i.seeds,
      now: T0,
    }
  }

  fn run(fake: &mut Fake, plan: &DryRunPlan) -> Result<RuleDryRun, DryRunRefusal> {
    futures::executor::block_on(run_dry_run(fake, plan))
  }

  #[test]
  fn a_dry_run_is_two_uploads_two_budget_calls_and_one_delete() {
    let i = inputs(true);
    let mut fake = Fake {
      budget: 9,
      ..Fake::default()
    };
    let result = run(&mut fake, &plan(&i)).expect("finished");
    let rule = uuid_str(Uuid::from_u128(RULE));
    let sibling = uuid_str(Uuid::from_u128(SIBLING));
    let first = &fake.calls[..3];
    assert_eq!(first[0], "allowed");
    assert_eq!(
      first[1],
      format!("upload Test main.js prelude.js pidgeiot.js rules.js r-{rule}-v3.js r-{rule}-v2.js")
    );
    assert!(first[2].starts_with("dispatch \"replay\" 4194304 30000"));
    assert_eq!(
      fake.bodies[0]["rules"],
      json!([{ "id": format!("{rule}-v3"), "version": 3 },
        { "id": format!("{rule}-v2"), "version": 2 }])
    );
    let upload2 = fake
      .calls
      .iter()
      .position(|c| c.starts_with("upload CpuCheck"))
      .expect("second upload");
    assert_eq!(fake.calls[upload2 - 1], "allowed");
    assert_eq!(
      fake.calls[upload2],
      format!("upload CpuCheck main.js prelude.js pidgeiot.js rules.js r-{sibling}.js r-{rule}.js")
    );
    let live: Vec<&Value> = fake.bodies.iter().filter(|b| b["mode"] == "live").collect();
    assert_eq!(live.len(), 2 * worked_events().len().min(CPU_CHECK_BATCHES));
    assert_eq!(
      live[0]["rules"].as_array().unwrap().len(),
      1,
      "the candidate alone first"
    );
    assert_eq!(live.last().unwrap()["rules"].as_array().unwrap().len(), 2);
    assert_eq!(fake.calls.last().map(String::as_str), Some("delete"));
    assert_eq!(fake.calls.iter().filter(|c| *c == "delete").count(), 1);
    assert_eq!(fake.calls.iter().filter(|c| *c == "allowed").count(), 2);
    assert_eq!(result.cpu.candidate, RuleCpuResult::Pass);
    assert_eq!(result.cpu.bundle, RuleCpuResult::Pass);
    assert!(result.deployed.is_some() && result.seeded.is_none());
  }

  #[test]
  fn a_cpu_kill_is_a_warning_and_the_dry_run_still_finishes() {
    let i = inputs(true);
    let mut fake = Fake {
      budget: 9,
      ..Fake::default()
    };
    // The replay answers quietly, then the candidate's first live batch loses its race.
    fake.outcomes.push_back(DispatchOutcome::Answered {
      status: 200,
      body: replay_body(json!([
        { "id": format!("{}-v3", uuid_str(Uuid::from_u128(RULE))), "status": "ok",
          "runs": [], "stopped": null, "final": {} },
        { "id": format!("{}-v2", uuid_str(Uuid::from_u128(RULE))), "status": "ok",
          "runs": [], "stopped": null, "final": {} }
      ]))
      .into_bytes(),
    });
    fake.outcomes.push_back(DispatchOutcome::Failed);
    let result = run(&mut fake, &plan(&i)).expect("a CPU kill refuses nothing");
    assert_eq!(result.cpu.candidate, RuleCpuResult::Over);
    assert_eq!(result.cpu.bundle, RuleCpuResult::Pass);
    assert!(result.cpu.warns());
    assert_eq!(fake.calls.last().map(String::as_str), Some("delete"));
  }

  #[test]
  fn a_spent_budget_uploads_nothing_and_a_spent_second_call_skips_the_cpu_check() {
    let i = inputs(false);
    let mut fake = Fake::default();
    assert_eq!(
      run(&mut fake, &plan(&i)),
      Err(DryRunRefusal::Busy(BUSY_SECS))
    );
    assert_eq!(fake.calls, ["allowed"]);

    let mut fake = Fake {
      budget: 1,
      ..Fake::default()
    };
    let result = run(&mut fake, &plan(&i)).expect("finished");
    assert_eq!(result.cpu, RuleCpuCheck::default());
    assert!(!fake.calls.iter().any(|c| c.starts_with("upload CpuCheck")));
    assert_eq!(fake.calls.last().map(String::as_str), Some("delete"));
  }

  #[test]
  fn a_refused_upload_dispatches_nothing_and_deletes_only_what_may_have_landed() {
    let i = inputs(false);
    let invalid = CfAnswer::Invalid("Uncaught Error: top".into());
    let mut fake = Fake {
      budget: 9,
      upload_answers: [Err(invalid.clone())].into(),
      ..Fake::default()
    };
    assert_eq!(
      run(&mut fake, &plan(&i)),
      Err(DryRunRefusal::Upload(invalid))
    );
    assert!(
      !fake
        .calls
        .iter()
        .any(|c| c.starts_with("dispatch") || c == "delete")
    );

    let mut fake = Fake {
      budget: 9,
      upload_answers: [Err(CfAnswer::Unsure)].into(),
      ..Fake::default()
    };
    assert!(run(&mut fake, &plan(&i)).is_err());
    assert_eq!(fake.calls.last().map(String::as_str), Some("delete"));
  }

  #[test]
  fn a_replay_that_does_not_finish_records_nothing_and_still_deletes() {
    let i = inputs(false);
    let mut fake = Fake {
      budget: 9,
      outcomes: [DispatchOutcome::Failed].into(),
      ..Fake::default()
    };
    assert_eq!(
      run(&mut fake, &plan(&i)),
      Err(DryRunRefusal::Unfinished(ReplayFailure::Lost))
    );
    assert_eq!(fake.calls.last().map(String::as_str), Some("delete"));
    assert!(!fake.calls.iter().any(|c| c.starts_with("upload CpuCheck")));
  }

  #[test]
  fn the_bundle_failing_to_load_under_the_live_limit_is_a_warning() {
    let i = inputs(true);
    let mut fake = Fake {
      budget: 9,
      upload_answers: [Ok(()), Err(CfAnswer::Invalid("startup CPU".into()))].into(),
      ..Fake::default()
    };
    let result = run(&mut fake, &plan(&i)).expect("finished");
    assert_eq!(result.cpu.candidate, RuleCpuResult::DidNotLoad);
    assert_eq!(result.cpu.load_error.as_deref(), Some("startup CPU"));
    assert!(result.cpu.warns());
  }

  #[test]
  fn live_state_seeds_a_second_replay_and_cases_run_first_from_empty_state() {
    let mut i = inputs(false);
    let mut seed = Seed::default();
    seed.flock.insert("n".into(), json!(1));
    i.seeds.insert(Uuid::from_u128(RULE), seed);
    i.cases = vec![case(&[]), case(&[])];
    let mut fake = Fake {
      budget: 9,
      ..Fake::default()
    };
    let result = run(&mut fake, &plan(&i)).expect("finished");
    let replays: Vec<&Value> = fake
      .bodies
      .iter()
      .filter(|b| b["mode"] == "replay")
      .collect();
    assert_eq!(replays.len(), 4, "two cases, the window, the seeded window");
    assert_eq!(replays[0]["events"][0]["pigeon"]["id"], "a".repeat(64));
    assert!(replays[0]["rules"][0].get("seed").is_none());
    assert_eq!(replays[3]["rules"][0]["seed"]["flock"], json!({ "n": 1 }));
    assert_eq!(result.cases.len(), 2);
    assert!(result.cases.iter().all(|c| c.passed));
    assert!(result.seeded.is_some());
    // The bundle is the candidate alone, so its side is the candidate's.
    assert_eq!(result.cpu.bundle, result.cpu.candidate);
  }

  #[test]
  fn a_bad_case_is_refused_before_anything_is_spent() {
    let mut i = inputs(false);
    i.cases = vec![case(&[])];
    let mut fake = Fake {
      budget: 9,
      ..Fake::default()
    };
    assert_eq!(
      run(&mut fake, &plan(&i)),
      Err(DryRunRefusal::Case(CaseError::CaseCount))
    );
    assert!(fake.calls.is_empty());
  }

  #[test]
  fn nothing_to_replay_still_loads_the_version_and_runs_no_cpu_check() {
    let mut i = inputs(false);
    i.events.clear();
    let mut fake = Fake {
      budget: 9,
      ..Fake::default()
    };
    let result = run(&mut fake, &plan(&i)).expect("finished");
    assert_eq!(fake.calls.len(), 3, "{:?}", fake.calls);
    assert_eq!(result.candidate.runs, 0);
    assert_eq!(result.cpu.candidate, RuleCpuResult::NotRun);
    assert!(!result.cpu.warns());
  }

  #[test]
  fn regrouped_history_replays_through_the_same_path() {
    let at = |s| OffsetDateTime::from_unix_timestamp(s).unwrap();
    let points: Vec<TelemetryHistoryPoint> = (0..3)
      .map(|n| TelemetryHistoryPoint {
        pigeon_id: "a".into(),
        key: "temp_c".into(),
        value: "41".into(),
        value_num: Some(41.0),
        reported_at: at(T0 + n),
      })
      .collect();
    let events = regroup_history(&points, &flock(), &HashMap::new());
    let body = replay_input(&events, &[want("k")]).expect("serializes");
    let input: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(input["events"][0]["readings"].as_array().unwrap().len(), 3);
  }
}
