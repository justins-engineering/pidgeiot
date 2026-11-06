//! The queue consumer's second pass over user-authored rules: per batch the cached gate, the
//! fresh read, one dispatch of the flock's script, and the record of what it answered.
//!
//! Blame follows where a failure happened, never what it says, since a rule's own throw reaches
//! dovecote with its text intact. A flock whose script was never uploaded is not dispatched, so a
//! missing script is never counted against it; a binding that cannot hand out the script is the
//! platform's and trips the breaker; anything once the request is on its way is the flock's.

use std::pin::pin;
use std::time::Duration;

use futures::StreamExt;
use futures::future::{Either, select};
use js_sys::{Function, Object, Promise, Reflect};
use tokio_postgres::Client;
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::{Date, Delay, Env, Response, console_error};

use crate::helpers::alerts::send_rule_email;
use crate::helpers::get_db_client;
use crate::helpers::rule_admin::rules_org_allowed;
use crate::helpers::rule_store::{
  RuleMail, RuleMailKind, ensure_rule_tables, fresh_read, mail_targets, record_runs, rules_gate,
};
use crate::helpers::rules::{
  Classified, DispatchOutcome, Lookup, PassLedger, PassVars, PreGate, RulePlan, RuleWork,
  RunStatus, ScriptKind, Skip, build_event, classify, deliverable_recipients, live_input,
  plan_record, read_cap, rule_mail_message, rule_paused_message, script_name, uuid_str,
};
#[cfg(test)]
use crate::helpers::{ResolvedReading, rules::pass_vars};

/// The most one dispatch may take, call and body read together, before it is aborted and counted
/// as the flock's. The platform's CPU kill can land past it, and then reads as this loss.
pub const DISPATCH_WALL_MS: u64 = 2_000;

/// Where the harness listens. The binding routes the request; the host is never resolved.
const RUN_URL: &str = "https://rules/run";

/// Most batches skipped for the budget whose watched rules a pass still records as `skipped`,
/// which bounds the writes past [`crate::helpers::rules::PASS_BUDGET_MS`].
pub const SKIPPED_RECORDS_PER_PASS: usize = 4;

/// Postgres failed, so the rest of the pass is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Down;

/// Everything the pass does outside itself, so each decision it makes is a host test.
pub(crate) trait PassIo {
  /// Milliseconds from any fixed origin.
  fn now_ms(&self) -> u64;
  /// The cached gate: whether the pigeon's flock has a live rule.
  async fn gate(&mut self, pigeon_id: &str) -> Result<bool, Down>;
  /// The fresh read; `None` for a pigeon gone or suspended.
  async fn read(&mut self, pigeon_id: &str) -> Result<Option<Lookup>, Down>;
  /// One dispatch of `script` for `flock_id`, its body read to at most `cap` + 1 bytes.
  async fn dispatch(
    &mut self,
    script: &str,
    flock_id: &str,
    body: &str,
    cap: usize,
  ) -> DispatchOutcome;
  /// Records the plans and sends the mail they claimed, also when a later rule's record failed.
  async fn record(
    &mut self,
    lookup: &Lookup,
    work: &RuleWork,
    plans: &[RulePlan],
    wall_ms: i32,
  ) -> Result<(), Down>;
}

/// What one pass did, for its one log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassCounts {
  /// Batches dispatched and recorded.
  pub dispatched: usize,
  /// Batches of a flock whose script was never uploaded.
  pub unscripted: usize,
  /// Batches of a flock whose organization `RULES_ALLOWED_ORG_IDS` does not list.
  pub unlisted: usize,
  /// Whole failures while our own deploy held the flock's lease, counted against no one.
  pub deploying: usize,
  /// Dispatches the platform failed.
  pub platform: usize,
  /// Batches skipped for [`crate::helpers::rules::PASS_BUDGET_MS`].
  pub budget: usize,
  /// Of those, the batches recorded as `skipped` on their watched rules.
  pub budget_recorded: usize,
  /// Batches skipped once the breaker tripped.
  pub breaker: usize,
  /// Batches skipped because their account's dispatch already failed or answered slowly.
  pub account: usize,
  /// Postgres failed and the rest were skipped.
  pub down: bool,
}

/// Runs each batch's rules: the field-pigeon gate and the breaker and budget before any
/// statement, then the gate, the fresh read, the organization allowlist, one dispatch and its
/// record.
///
/// A batch the budget skips reads nothing more, but when the pass already read its pigeon it
/// leaves a `skipped` run on each watched rule, for up to [`SKIPPED_RECORDS_PER_PASS`] batches,
/// so a watch shows the batch the rules never saw.
pub(crate) async fn run_pass<I: PassIo>(
  io: &mut I,
  vars: &PassVars,
  work: &[RuleWork],
) -> PassCounts {
  let mut ledger = PassLedger::new(io.now_ms());
  let mut counts = PassCounts::default();
  let mut read: Vec<(&str, Lookup)> = Vec::new();
  for item in work {
    if vars.gate(&item.pigeon_id) != PreGate::Lookup {
      continue;
    }
    if ledger.tripped() {
      counts.breaker += 1;
      continue;
    }
    if ledger.budget_spent(io.now_ms()) {
      counts.budget += 1;
      let earlier = read
        .iter()
        .find(|(pigeon, _)| *pigeon == item.pigeon_id)
        .map(|(_, lookup)| lookup.clone());
      if let Some(lookup) = earlier
        && record_skipped(io, &lookup, item, &mut counts)
          .await
          .is_err()
      {
        counts.down = true;
        break;
      }
      continue;
    }
    let found = match io.gate(&item.pigeon_id).await {
      Ok(true) => io.read(&item.pigeon_id).await,
      Ok(false) => Ok(None),
      Err(down) => Err(down),
    };
    let lookup = match found {
      Ok(Some(lookup)) if !lookup.rules.is_empty() => lookup,
      Ok(_) => continue,
      Err(Down) => {
        counts.down = true;
        break;
      }
    };
    if !rules_org_allowed(vars.allowed_orgs.as_deref(), lookup.org) {
      counts.unlisted += 1;
      continue;
    }
    if lookup.uploaded_at.is_none() {
      counts.unscripted += 1;
      continue;
    }
    let account = uuid_str(lookup.account);
    match ledger.may_dispatch(&account, io.now_ms()) {
      Ok(()) => {}
      Err(Skip::Breaker) => {
        counts.breaker += 1;
        continue;
      }
      Err(Skip::AccountFailed) => {
        counts.account += 1;
        continue;
      }
      Err(Skip::Budget) => {
        counts.budget += 1;
        if record_skipped(io, &lookup, item, &mut counts)
          .await
          .is_err()
        {
          counts.down = true;
          break;
        }
        continue;
      }
    }
    if !read.iter().any(|(pigeon, _)| *pigeon == item.pigeon_id) {
      read.push((&item.pigeon_id, lookup.clone()));
    }
    let sent = lookup.dispatch_rules();
    let common = build_event(
      item,
      lookup.flock.clone(),
      lookup.pigeon.clone(),
      lookup.now,
    );
    let (Some(body), Some(script)) = (
      live_input(&common, &sent),
      script_name(ScriptKind::Flock, &lookup.flock.id),
    ) else {
      continue;
    };
    let started = io.now_ms();
    let outcome = io
      .dispatch(&script, &lookup.flock.id, &body, read_cap(sent.len()))
      .await;
    let elapsed = io.now_ms().saturating_sub(started);
    let wall_ms = i32::try_from(elapsed).unwrap_or(i32::MAX);
    let classified = classify(&sent, outcome);
    if lookup.leased && matches!(classified, Classified::Flock(_)) {
      counts.deploying += 1;
      continue;
    }
    ledger.record(&account, &classified, elapsed);
    if classified == Classified::Platform {
      counts.platform += 1;
      continue;
    }
    let plans = plan_record(&lookup, &item.readings, &classified);
    if io.record(&lookup, item, &plans, wall_ms).await.is_err() {
      counts.down = true;
      break;
    }
    counts.dispatched += 1;
  }
  counts
}

/// Records a batch the budget skipped as `skipped` on its watched rules; the plan writes nothing
/// for a rule that is not watched, and nothing past [`SKIPPED_RECORDS_PER_PASS`] batches.
async fn record_skipped<I: PassIo>(
  io: &mut I,
  lookup: &Lookup,
  item: &RuleWork,
  counts: &mut PassCounts,
) -> Result<(), Down> {
  if counts.budget_recorded >= SKIPPED_RECORDS_PER_PASS || !lookup.rules.iter().any(|r| r.watched) {
    return Ok(());
  }
  let plans = plan_record(
    lookup,
    &item.readings,
    &Classified::Flock(RunStatus::Skipped),
  );
  io.record(lookup, item, &plans, 0).await?;
  counts.budget_recorded += 1;
  Ok(())
}

/// The pass against the Worker's own bindings, opening Postgres at the first batch that needs it.
pub(crate) struct LivePass<'a> {
  env: &'a Env,
  client: Option<Client>,
}

impl<'a> LivePass<'a> {
  /// A pass that has opened nothing yet.
  pub(crate) fn new(env: &'a Env) -> Self {
    Self { env, client: None }
  }

  async fn client(&mut self) -> Result<&mut Client, Down> {
    if self.client.is_none() {
      let Ok(client) = get_db_client(self.env).await else {
        return Err(Down);
      };
      if ensure_rule_tables(&client).await.is_err() {
        return Err(Down);
      }
      self.client = Some(client);
    }
    self.client.as_mut().ok_or(Down)
  }
}

/// Logs a failed statement by its SQLSTATE alone: an error's text can quote a row's values.
fn down(step: &str, error: &tokio_postgres::Error) -> Down {
  let code = error.code().map_or("none", |code| code.code());
  console_error!("rules: the pass's {step} failed, SQLSTATE {code}");
  Down
}

impl PassIo for LivePass<'_> {
  fn now_ms(&self) -> u64 {
    Date::now().as_millis()
  }

  async fn gate(&mut self, pigeon_id: &str) -> Result<bool, Down> {
    let client = self.client().await?;
    rules_gate(client, pigeon_id)
      .await
      .map_err(|e| down("gate", &e))
  }

  async fn read(&mut self, pigeon_id: &str) -> Result<Option<Lookup>, Down> {
    let client = self.client().await?;
    fresh_read(client, pigeon_id)
      .await
      .map_err(|e| down("read", &e))
  }

  async fn dispatch(
    &mut self,
    script: &str,
    flock_id: &str,
    body: &str,
    cap: usize,
  ) -> DispatchOutcome {
    dispatch(self.env, script, flock_id, body, cap, DISPATCH_WALL_MS).await
  }

  async fn record(
    &mut self,
    lookup: &Lookup,
    work: &RuleWork,
    plans: &[RulePlan],
    wall_ms: i32,
  ) -> Result<(), Down> {
    let env = self.env;
    let client = self.client().await?;
    let (mails, failed) = record_runs(client, lookup, work, plans, wall_ms).await;
    if !mails.is_empty() {
      send_rule_mails(env, client, lookup, &mails).await;
    }
    match failed {
      Some(e) => Err(down("record", &e)),
      None => Ok(()),
    }
  }
}

/// Sends mail a committed run claimed. A send that fails is logged and, like alert mail, not
/// retried.
async fn send_rule_mails(env: &Env, client: &Client, lookup: &Lookup, mails: &[RuleMail]) {
  let mut rules: Vec<Uuid> = mails.iter().map(|mail| mail.rule_id).collect();
  rules.sort_unstable();
  rules.dedup();
  let targets = match mail_targets(client, &rules).await {
    Ok(targets) => targets,
    Err(e) => {
      down("mail read", &e);
      return;
    }
  };
  for mail in mails {
    let Some(target) = targets.iter().find(|t| t.rule_id == mail.rule_id) else {
      continue;
    };
    let rule = uuid_str(mail.rule_id);
    let (to, (subject, text)) = match &mail.kind {
      RuleMailKind::Alert(alert) => {
        let (to, refused) = deliverable_recipients(
          &target.recipients,
          target.owner_email.as_deref(),
          target.member_emails.as_deref(),
        );
        if refused > 0 {
          console_error!(
            "rules: rule {rule} dropped {refused} recipient(s) not tied to its account"
          );
        }
        let message =
          rule_mail_message(alert, &target.name, &lookup.flock.name, &lookup.pigeon.name);
        (to, message)
      }
      // The author's address only while they are a member; the flock owner's once they leave.
      RuleMailKind::Paused { reason, error } => {
        let to = target
          .author_email
          .clone()
          .or_else(|| target.owner_email.clone());
        let message =
          rule_paused_message(&target.name, &lookup.flock.name, reason, error.as_deref());
        (to.into_iter().collect(), message)
      }
    };
    for address in &to {
      if send_rule_email(env, address, &subject, &text, &rule)
        .await
        .is_err()
      {
        console_error!("rules: a mail of rule {rule} was not sent");
      }
    }
  }
}

/// Dispatches one rules script: the call and the body read raced against `wall_ms`
/// ([`DISPATCH_WALL_MS`] live) and aborted when they lose.
pub(crate) async fn dispatch(
  env: &Env,
  script: &str,
  flock_id: &str,
  body: &str,
  cap: usize,
  wall_ms: u64,
) -> DispatchOutcome {
  let Some(fetcher) = rules_fetcher(env, script, flock_id) else {
    return DispatchOutcome::Binding;
  };
  let Ok(controller) = web_sys::AbortController::new() else {
    return DispatchOutcome::Binding;
  };
  let signal = controller.signal();
  let call = pin!(send(&fetcher, body, &signal, cap));
  let wall = pin!(Delay::from(Duration::from_millis(wall_ms)));
  let outcome = match select(call, wall).await {
    Either::Left((outcome, _)) => outcome,
    Either::Right(((), _)) => DispatchOutcome::Failed,
  };
  // Also releases a body left unread past the cap.
  controller.abort();
  outcome
}

/// A rules script through the `RULES` binding's own three-argument `get`, which the `worker`
/// crate's `DynamicDispatcher::get` does not expose. `outbound.flock_id` is always set: the
/// binding refuses a `get` without it, and it is how the outbound Worker names who tried. No
/// per-call limits: the platform ignores them and the upload metadata's bind. `None` is the
/// platform's failure, since a missing script surfaces at the fetch, not here.
fn rules_fetcher(env: &Env, script: &str, flock_id: &str) -> Option<JsValue> {
  let ns = Reflect::get(env, &JsValue::from_str("RULES"))
    .ok()
    .filter(JsValue::is_object)?;
  let get = Reflect::get(&ns, &JsValue::from_str("get"))
    .ok()?
    .dyn_into::<Function>()
    .ok()?;
  let outbound = Object::new();
  let options = Object::new();
  if !(set(&outbound, "flock_id", &JsValue::from_str(flock_id))
    && set(&options, "outbound", &outbound))
  {
    return None;
  }
  get
    .call3(&ns, &JsValue::from_str(script), &Object::new(), &options)
    .ok()
    .filter(JsValue::is_object)
}

/// Sets one property of a plain object built for the binding.
fn set(obj: &Object, key: &str, value: &JsValue) -> bool {
  Reflect::set(obj, &JsValue::from_str(key), value).unwrap_or(false)
}

/// Posts `body` to the script and reads its answer. A fetch that cannot be made is the
/// platform's; one that rejects is the flock's, whatever its message, which is never read.
async fn send(
  fetcher: &JsValue,
  body: &str,
  signal: &web_sys::AbortSignal,
  cap: usize,
) -> DispatchOutcome {
  let Some(fetch) = Reflect::get(fetcher, &JsValue::from_str("fetch"))
    .ok()
    .and_then(|f| f.dyn_into::<Function>().ok())
  else {
    return DispatchOutcome::Binding;
  };
  let init = Object::new();
  let headers = Object::new();
  let built = set(
    &headers,
    "content-type",
    &JsValue::from_str("application/json"),
  ) && set(&init, "method", &JsValue::from_str("POST"))
    && set(&init, "headers", &headers)
    && set(&init, "body", &JsValue::from_str(body))
    && set(&init, "signal", signal);
  if !built {
    return DispatchOutcome::Binding;
  }
  let Some(promise) = fetch
    .call2(fetcher, &JsValue::from_str(RUN_URL), &init)
    .ok()
    .and_then(|p| p.dyn_into::<Promise>().ok())
  else {
    return DispatchOutcome::Binding;
  };
  let Some(answer) = JsFuture::from(promise)
    .await
    .ok()
    .and_then(|a| a.dyn_into::<web_sys::Response>().ok())
  else {
    return DispatchOutcome::Failed;
  };
  let mut response = Response::from(answer);
  let status = response.status_code();
  let mut read = Vec::new();
  if let Ok(mut stream) = response.stream() {
    while let Some(chunk) = stream.next().await {
      let Ok(chunk) = chunk else {
        return DispatchOutcome::Failed;
      };
      if take_capped(&mut read, &chunk, cap) {
        break;
      }
    }
  }
  DispatchOutcome::Answered { status, body: read }
}

/// Appends `chunk` to `body` up to `cap` + 1 bytes, one past the cap being enough to call the
/// answer oversize. True once full.
pub(crate) fn take_capped(body: &mut Vec<u8>, chunk: &[u8], cap: usize) -> bool {
  let room = cap.saturating_add(1).saturating_sub(body.len());
  body.extend_from_slice(&chunk[..chunk.len().min(room)]);
  body.len() > cap
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::helpers::rules::{FIELD_PIGEONS, LiveRule, PASS_BUDGET_MS, uuid_str};
  use capsules::{RuleNamedRef, RuleState};
  use serde_json::{Value, json};
  use std::collections::HashMap;

  const T0: i64 = 1_793_380_000;

  #[derive(Default)]
  struct Fake {
    clock: u64,
    /// How long each dispatch takes on the fake clock.
    dispatch_ms: u64,
    lookups: HashMap<String, Lookup>,
    /// What each flock's dispatch answers; absent means a 200 from the harness for every rule.
    outcomes: HashMap<String, DispatchOutcome>,
    down: bool,
    gated: Vec<String>,
    dispatched: Vec<(String, String, Value, usize)>,
    recorded: Vec<(String, Vec<(Uuid, RunStatus)>, i32)>,
  }

  impl PassIo for Fake {
    fn now_ms(&self) -> u64 {
      self.clock
    }

    async fn gate(&mut self, pigeon_id: &str) -> Result<bool, Down> {
      if self.down {
        return Err(Down);
      }
      self.gated.push(pigeon_id.to_string());
      Ok(self.lookups.contains_key(pigeon_id))
    }

    async fn read(&mut self, pigeon_id: &str) -> Result<Option<Lookup>, Down> {
      Ok(self.lookups.get(pigeon_id).cloned())
    }

    async fn dispatch(
      &mut self,
      script: &str,
      flock_id: &str,
      body: &str,
      cap: usize,
    ) -> DispatchOutcome {
      let request: Value = serde_json::from_str(body).expect("the request is JSON");
      self.clock += self.dispatch_ms;
      self.dispatched.push((
        script.to_string(),
        flock_id.to_string(),
        request.clone(),
        cap,
      ));
      if let Some(outcome) = self.outcomes.get(flock_id) {
        return outcome.clone();
      }
      let results: Vec<Value> = request["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .map(|r| json!({ "id": r["id"], "status": "ok", "out_json": "{}", "logs": [] }))
        .collect();
      DispatchOutcome::Answered {
        status: 200,
        body: json!({ "contract": 1, "results": results })
          .to_string()
          .into_bytes(),
      }
    }

    async fn record(
      &mut self,
      lookup: &Lookup,
      _work: &RuleWork,
      plans: &[RulePlan],
      wall_ms: i32,
    ) -> Result<(), Down> {
      let statuses = plans.iter().map(|p| (p.rule_id, p.status)).collect();
      self
        .recorded
        .push((lookup.flock.id.clone(), statuses, wall_ms));
      Ok(())
    }
  }

  fn flock(n: u128) -> Uuid {
    Uuid::from_u128(0xf000 + n)
  }

  /// A pigeon of flock `n` owned by account `account`, with `rules` live rules and an upload.
  fn found(n: u128, account: u128, rules: u128) -> Lookup {
    Lookup {
      now: T0,
      flock: RuleNamedRef {
        id: uuid_str(flock(n)),
        name: "F".to_string(),
      },
      pigeon: RuleNamedRef {
        id: "p".to_string(),
        name: "P".to_string(),
      },
      account: Uuid::from_u128(account),
      org: Some(Uuid::from_u128(account)),
      uploaded_at: Some(T0 - 3_600),
      leased: false,
      account_mail_day: 0,
      platform_mail_day: 0,
      rules: (0..rules)
        .map(|i| LiveRule {
          id: Uuid::from_u128(n * 100 + i),
          version: 1,
          state: RuleState::default(),
          flags: Default::default(),
          mail: Default::default(),
          failing: false,
          watched: false,
          pending: false,
        })
        .collect(),
    }
  }

  fn work(pigeon: &str) -> RuleWork {
    let metrics = [("k".to_string(), "1".to_string())].into_iter().collect();
    RuleWork::new(
      pigeon,
      pigeon,
      false,
      vec![ResolvedReading::new(T0, metrics)],
    )
    .expect("a reading")
  }

  /// The switch on and organizations 1 to 9 listed.
  fn vars() -> PassVars {
    let listed: Vec<String> = (1..10).map(|n| uuid_str(Uuid::from_u128(n))).collect();
    pass_vars(|name| match name {
      "RULES_DISPATCH" => Some("on".to_string()),
      "RULES_ALLOWED_ORG_IDS" => Some(listed.join(",")),
      _ => None,
    })
    .expect("on")
  }

  fn recorded_flocks(fake: &Fake) -> Vec<&str> {
    fake.recorded.iter().map(|(f, _, _)| f.as_str()).collect()
  }

  fn run(fake: &mut Fake, batches: &[&str]) -> PassCounts {
    let work: Vec<RuleWork> = batches.iter().map(|p| work(p)).collect();
    futures::executor::block_on(run_pass(fake, &vars(), &work))
  }

  #[test]
  fn a_dispatch_names_its_own_flock_and_script_and_each_rule_in_order() {
    let mut fake = Fake::default();
    fake.lookups.insert("p1".into(), found(1, 1, 2));
    let counts = run(&mut fake, &["p1"]);
    assert_eq!(counts.dispatched, 1);
    let (script, flock_id, request, cap) = &fake.dispatched[0];
    let mut expected = String::from("flock-");
    expected.push_str(&uuid_str(flock(1)).replace('-', ""));
    assert_eq!(script, &expected);
    assert_eq!(
      flock_id,
      &uuid_str(flock(1)),
      "outbound.flock_id is the flock's own"
    );
    assert_eq!(*cap, read_cap(2));
    assert_eq!(request["mode"], "live");
    assert_eq!(request["common"]["flock"]["id"], json!(uuid_str(flock(1))));
    let ids: Vec<&str> = request["rules"]
      .as_array()
      .expect("rules")
      .iter()
      .map(|r| r["id"].as_str().expect("id"))
      .collect();
    assert_eq!(
      ids,
      [
        uuid_str(Uuid::from_u128(100)),
        uuid_str(Uuid::from_u128(101))
      ]
    );
    let (_, statuses, _) = &fake.recorded[0];
    assert!(statuses.iter().all(|(_, s)| *s == RunStatus::Ok));
  }

  #[test]
  fn a_field_pigeon_is_never_read_and_a_flock_with_no_upload_never_dispatched() {
    let mut fake = Fake::default();
    fake.lookups.insert(FIELD_PIGEONS[0].into(), found(1, 1, 1));
    let mut never = found(2, 2, 1);
    never.uploaded_at = None;
    fake.lookups.insert("p2".into(), never);
    let counts = run(&mut fake, &[FIELD_PIGEONS[0], "p2"]);
    assert_eq!(fake.gated, ["p2"]);
    assert!(fake.dispatched.is_empty() && fake.recorded.is_empty());
    assert_eq!(
      counts.unscripted, 1,
      "no script is ours to fix, never the flock's failure"
    );
  }

  #[test]
  fn a_platform_failure_trips_the_breaker_and_records_nothing() {
    let mut fake = Fake::default();
    fake.lookups.insert("p1".into(), found(1, 1, 1));
    fake.lookups.insert("p2".into(), found(2, 2, 1));
    fake
      .outcomes
      .insert(uuid_str(flock(1)), DispatchOutcome::Binding);
    let counts = run(&mut fake, &["p1", "p2"]);
    assert!(fake.recorded.is_empty());
    assert_eq!(
      fake.gated,
      ["p1"],
      "nothing after the trip runs a statement"
    );
    assert_eq!((counts.platform, counts.breaker), (1, 1));
  }

  #[test]
  fn two_accounts_failing_trip_the_breaker_one_account_cannot() {
    let mut fake = Fake::default();
    for (pigeon, n, account) in [("p1", 1, 1), ("p2", 2, 1), ("p3", 3, 2), ("p4", 4, 3)] {
      fake.lookups.insert(pigeon.into(), found(n, account, 2));
    }
    for n in [1, 2, 3] {
      fake
        .outcomes
        .insert(uuid_str(flock(n)), DispatchOutcome::Failed);
    }
    let counts = run(&mut fake, &["p1", "p2", "p3", "p4"]);
    assert_eq!(
      recorded_flocks(&fake),
      [uuid_str(flock(1)), uuid_str(flock(3))]
    );
    for (_, statuses, _) in &fake.recorded {
      assert_eq!(statuses.len(), 2);
      assert!(
        statuses
          .iter()
          .all(|(_, s)| *s == RunStatus::DispatchFailed)
      );
    }
    assert_eq!((counts.account, counts.breaker), (1, 1));
    assert!(!fake.gated.contains(&"p4".to_string()));
  }

  #[test]
  fn an_organization_the_allowlist_does_not_name_is_read_but_never_dispatched() {
    let mut fake = Fake::default();
    fake.lookups.insert("p1".into(), found(1, 1, 1));
    fake.lookups.insert("p2".into(), found(2, 99, 1));
    let mut personal = found(3, 3, 1);
    personal.org = None;
    fake.lookups.insert("p3".into(), personal);
    let counts = run(&mut fake, &["p1", "p2", "p3"]);
    assert_eq!(fake.gated, ["p1", "p2", "p3"]);
    assert_eq!(recorded_flocks(&fake), [uuid_str(flock(1))]);
    assert_eq!(fake.dispatched.len(), 1);
    assert_eq!((counts.dispatched, counts.unlisted), (1, 2));

    let mut fake = Fake::default();
    fake.lookups.insert("p1".into(), found(1, 1, 1));
    let unset = pass_vars(|name| (name == "RULES_DISPATCH").then(|| "on".to_string()));
    let counts =
      futures::executor::block_on(run_pass(&mut fake, &unset.expect("on"), &[work("p1")]));
    assert!(
      fake.dispatched.is_empty(),
      "an unset allowlist lists no one"
    );
    assert_eq!(counts.unlisted, 1);
  }

  #[test]
  fn a_slow_answer_skips_its_accounts_other_flocks_but_not_another_account() {
    let mut fake = Fake {
      dispatch_ms: 1_500,
      ..Fake::default()
    };
    for (pigeon, n, account) in [("p1", 1, 1), ("p2", 2, 1), ("p3", 3, 2)] {
      fake.lookups.insert(pigeon.into(), found(n, account, 1));
    }
    let counts = run(&mut fake, &["p1", "p2", "p3"]);
    assert_eq!(
      recorded_flocks(&fake),
      [uuid_str(flock(1)), uuid_str(flock(3))]
    );
    assert_eq!((counts.account, counts.budget, counts.breaker), (1, 0, 0));
    for (_, statuses, _) in &fake.recorded {
      assert!(
        statuses.iter().all(|(_, s)| *s == RunStatus::Ok),
        "slow is no failure"
      );
    }
  }

  #[test]
  fn a_whole_failure_during_our_deploy_counts_against_nobody() {
    let mut fake = Fake::default();
    let mut deploying = found(1, 1, 1);
    deploying.leased = true;
    fake.lookups.insert("p1".into(), deploying);
    fake.lookups.insert("p2".into(), found(2, 1, 1));
    fake.lookups.insert("p3".into(), found(3, 2, 1));
    for n in [1, 3] {
      fake
        .outcomes
        .insert(uuid_str(flock(n)), DispatchOutcome::Failed);
    }
    let counts = run(&mut fake, &["p1", "p2", "p3"]);
    assert_eq!(
      recorded_flocks(&fake),
      [uuid_str(flock(2)), uuid_str(flock(3))]
    );
    assert_eq!(
      (counts.deploying, counts.account, counts.breaker),
      (1, 0, 0)
    );
  }

  #[test]
  fn an_answer_past_the_cap_is_the_flocks_oversize() {
    let mut fake = Fake::default();
    fake.lookups.insert("p1".into(), found(1, 1, 1));
    let body = vec![b' '; read_cap(1) + 1];
    let answer = DispatchOutcome::Answered { status: 200, body };
    fake.outcomes.insert(uuid_str(flock(1)), answer);
    run(&mut fake, &["p1"]);
    assert_eq!(fake.recorded[0].1[0].1, RunStatus::Oversize);
  }

  #[test]
  fn the_budget_stops_new_dispatches_and_a_dispatch_records_its_wall() {
    let mut fake = Fake {
      dispatch_ms: PASS_BUDGET_MS / 2,
      ..Fake::default()
    };
    for (pigeon, n) in [("p1", 1), ("p2", 2), ("p3", 3)] {
      fake.lookups.insert(pigeon.into(), found(n, n, 1));
    }
    let counts = run(&mut fake, &["p1", "p2", "p3"]);
    assert_eq!((counts.dispatched, counts.budget), (2, 1));
    assert_eq!(
      fake.recorded[0].2,
      i32::try_from(PASS_BUDGET_MS / 2).expect("small")
    );
  }

  #[test]
  fn a_batch_the_budget_skips_is_recorded_skipped_on_watched_rules_without_another_read() {
    let mut fake = Fake {
      dispatch_ms: PASS_BUDGET_MS,
      ..Fake::default()
    };
    let mut watched = found(1, 1, 2);
    watched.rules[1].watched = true;
    fake.lookups.insert("p1".into(), watched);
    fake.lookups.insert("p2".into(), found(2, 2, 1));
    let counts = run(&mut fake, &["p1", "p1", "p2"]);
    assert_eq!(fake.gated, ["p1"], "nothing past the budget reads");
    assert_eq!(
      (counts.dispatched, counts.budget, counts.budget_recorded),
      (1, 2, 1)
    );
    let (_, statuses, wall) = &fake.recorded[1];
    assert_eq!(
      statuses,
      &[
        (Uuid::from_u128(100), RunStatus::Skipped),
        (Uuid::from_u128(101), RunStatus::Skipped)
      ]
    );
    assert_eq!(*wall, 0);
    assert_eq!(
      fake.recorded.len(),
      2,
      "p2 was never read, so it records nothing"
    );

    let mut fake = Fake {
      dispatch_ms: PASS_BUDGET_MS,
      ..Fake::default()
    };
    fake.lookups.insert("p1".into(), found(1, 1, 1));
    let counts = run(&mut fake, &["p1", "p1"]);
    assert_eq!(
      (counts.budget, counts.budget_recorded),
      (1, 0),
      "no watched rule, no write"
    );
    assert_eq!(fake.recorded.len(), 1);

    let mut fake = Fake {
      dispatch_ms: PASS_BUDGET_MS,
      ..Fake::default()
    };
    let mut watched = found(1, 1, 1);
    watched.rules[0].watched = true;
    fake.lookups.insert("p1".into(), watched);
    let batches = vec!["p1"; 2 + SKIPPED_RECORDS_PER_PASS];
    let counts = run(&mut fake, &batches);
    assert_eq!(
      counts.budget_recorded, SKIPPED_RECORDS_PER_PASS,
      "bounded past the budget"
    );
  }

  #[test]
  fn postgres_down_ends_the_pass() {
    let mut fake = Fake {
      down: true,
      ..Fake::default()
    };
    fake.lookups.insert("p1".into(), found(1, 1, 1));
    let counts = run(&mut fake, &["p1", "p1"]);
    assert!(counts.down && fake.dispatched.is_empty());
  }

  #[test]
  fn a_body_is_read_to_one_byte_past_its_cap() {
    let mut body = Vec::new();
    assert!(!take_capped(&mut body, b"abc", 4));
    assert!(take_capped(&mut body, b"defgh", 4));
    assert_eq!(body, b"abcde");
    assert!(take_capped(&mut body, b"z", 4));
    assert_eq!(body.len(), 5);
  }
}
