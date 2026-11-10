//! The statements behind the dashboard's rule routes, and the refusals they decide on.
//!
//! Every statement names the rule together with the flock a [`FlockAccess`] proved, so a rule id
//! from another flock reads as absent. A read that gates a mutation selects `now()`, which
//! Hyperdrive never caches, and runs inside the mutation's own transaction under the rule's row
//! lock, keeping the lock order of [`super::rule_store`]: an enable locks the flock's row first,
//! to count its enabled rules without a racing enable, then the rule's.

use capsules::{
  MAX_ENABLED_RULES_PER_FLOCK, MAX_RULE_RUNS_PER_PAGE, MAX_RULE_VALUES_RETURNED, MAX_RULE_VERSIONS,
  PreparedRuleVersion, RULE_CONTRACT, Rule, RuleDetail, RuleRow, RuleRun, RuleRunRow, RuleValue,
  RuleValueRow, RuleVersion, RuleVersionRow,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use tokio_postgres::{Client, Row, Transaction, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

use crate::helpers::firmware::FlockAccess;
use crate::helpers::nidd::org_listed;
use crate::helpers::rule_dry_run::Seed;
use crate::helpers::rule_namespace::SweepBundle;
use crate::helpers::rule_store::{
  begin, end, ensure_rule_tables, release_deploy_lease, take_deploy_lease,
};
use crate::helpers::rules::{account_tag, uuid_str};
use crate::helpers::sha256_hex;

macro_rules! rule_columns {
  () => {
    "r.id, r.flock_id, r.created_by, r.name, r.recipients::text AS recipients, r.enabled,
     r.deployed_version, r.pending_version, r.paused_reason, r.blocked_at, r.fail_count,
     r.watch_until, r.created_at, r.updated_at"
  };
}

/// A version's columns but its test summary and source, which only a single read carries.
macro_rules! version_columns {
  () => {
    "v.rule_id, v.version, v.sha256, v.contract, v.origin, v.based_on, v.repair_depth, v.prompt,
     v.tested_sha256, v.tested_contract, v.tested_at, v.created_by, v.created_at"
  };
}

/// Why an enable was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnableRefusal {
  /// Nothing deployed: the flock's script holds no module for this rule.
  NotDeployed,
  /// Blocked by the platform, which only the platform lifts.
  Blocked,
  /// The flock already runs [`MAX_ENABLED_RULES_PER_FLOCK`] rules.
  FlockFull,
}

impl EnableRefusal {
  /// The route's status and text: 403 for the platform's block, 409 for the rule's own state.
  pub fn response(self) -> (u16, &'static str) {
    match self {
      EnableRefusal::NotDeployed => (409, "Conflict: deploy a version before enabling this rule"),
      EnableRefusal::Blocked => (403, BLOCKED),
      EnableRefusal::FlockFull => (409, FLOCK_FULL),
    }
  }
}

/// Why a test or a deploy was refused before anything was uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployRefusal {
  /// Blocked by the platform; a test would put the code back into the namespace.
  Blocked,
  /// No dry run of these bytes under the harness this build ships.
  Untested,
  /// Deploying enables the rule, and the flock already runs [`MAX_ENABLED_RULES_PER_FLOCK`].
  FlockFull,
}

impl DeployRefusal {
  /// The route's status and text.
  pub fn response(self) -> (u16, &'static str) {
    match self {
      DeployRefusal::Blocked => (403, BLOCKED),
      DeployRefusal::Untested => (409, "Conflict: test this version before deploying it"),
      DeployRefusal::FlockFull => (409, FLOCK_FULL),
    }
  }
}

const BLOCKED: &str = "Forbidden: this rule is blocked by the platform";
const FLOCK_FULL: &str = "Conflict: a flock runs at most 20 enabled rules";

/// The answer of every rule route on a flock whose organization `RULES_ALLOWED_ORG_IDS` does not
/// list.
pub const RULES_ORG_REFUSED: (u16, &str) = (
  403,
  "Forbidden: rules are not enabled for this organization",
);

/// Whether `listed`, the raw `RULES_ALLOWED_ORG_IDS`, names the flock's organization. Unset or
/// empty names none and a personal flock has none, so rules stay shut until an organization is
/// listed: rule source is customer code, stored only where the platform has opened rules.
pub fn rules_org_allowed(listed: Option<&str>, org: Option<Uuid>) -> bool {
  match (listed, org) {
    (Some(raw), Some(org)) => org_listed(raw, &org),
    _ => false,
  }
}

/// Whether enabling `rule` is refused, given how many other rules of its flock are enabled. An
/// already enabled rule is never refused, so a repeated enable is a no-op.
pub fn enable_refusal(rule: &RuleRow, enabled_siblings: i64) -> Option<EnableRefusal> {
  if rule.enabled {
    None
  } else if rule.deployed_version.is_none() {
    Some(EnableRefusal::NotDeployed)
  } else if rule.blocked_at.is_some() {
    Some(EnableRefusal::Blocked)
  } else if enabled_siblings >= MAX_ENABLED_RULES_PER_FLOCK {
    Some(EnableRefusal::FlockFull)
  } else {
    None
  }
}

/// Whether testing a version of the rule is refused: a test uploads the version's code, so a
/// platform block refuses it as it refuses a deploy.
pub fn test_refusal(release: &Release) -> Option<DeployRefusal> {
  release
    .rule
    .blocked_at
    .is_some()
    .then_some(DeployRefusal::Blocked)
}

/// Whether deploying `version` of `rule` is refused: blocked, not tested at these bytes under
/// [`RULE_CONTRACT`], or would enable one rule too many. Deploying enables the rule.
pub fn deploy_refusal(release: &Release) -> Option<DeployRefusal> {
  let Release {
    rule,
    version,
    enabled_siblings,
  } = release;
  let tested = version.tested_sha256.as_deref() == Some(version.sha256.as_str())
    && version.tested_contract == Some(RULE_CONTRACT as i32);
  if rule.blocked_at.is_some() {
    Some(DeployRefusal::Blocked)
  } else if !tested {
    Some(DeployRefusal::Untested)
  } else if !rule.enabled && *enabled_siblings >= MAX_ENABLED_RULES_PER_FLOCK {
    Some(DeployRefusal::FlockFull)
  } else {
    None
  }
}

fn rule_row(row: &Row) -> RuleRow {
  RuleRow {
    id: row.get("id"),
    flock_id: row.get("flock_id"),
    created_by: row.get("created_by"),
    name: row.get("name"),
    recipients: row.get("recipients"),
    enabled: row.get("enabled"),
    deployed_version: row.get("deployed_version"),
    pending_version: row.get("pending_version"),
    paused_reason: row.get("paused_reason"),
    blocked_at: row.get("blocked_at"),
    fail_count: row.get("fail_count"),
    watch_until: row.get("watch_until"),
    created_at: row.get("created_at"),
    updated_at: row.get("updated_at"),
  }
}

fn version_row(row: &Row) -> RuleVersionRow {
  RuleVersionRow {
    rule_id: row.get("rule_id"),
    version: row.get("version"),
    sha256: row.get("sha256"),
    contract: row.get("contract"),
    origin: row.get("origin"),
    based_on: row.get("based_on"),
    repair_depth: row.get("repair_depth"),
    prompt: row.get("prompt"),
    tested_sha256: row.get("tested_sha256"),
    tested_contract: row.get("tested_contract"),
    tested_at: row.get("tested_at"),
    test_summary: row.try_get("test_summary").ok().flatten(),
    created_by: row.get("created_by"),
    created_at: row.get("created_at"),
    source: row.try_get("source").ok().flatten(),
    builder_model: row.try_get("builder_model").ok().flatten(),
  }
}

fn run_row(row: &Row) -> RuleRunRow {
  RuleRunRow {
    rule_id: row.get("rule_id"),
    version: row.get("version"),
    pigeon_id: row.get("pigeon_id"),
    batch_id: row.get("batch_id"),
    status: row.get("status"),
    readings: row.get("readings"),
    alerts_sent: row.get("alerts_sent"),
    alerts_suppressed: row.get("alerts_suppressed"),
    values_written: row.get("values_written"),
    state_changed: row.get("state_changed"),
    wall_ms: row.get("wall_ms"),
    error: row.get("error"),
    logs: row.get("logs"),
    ran_at: row.get("ran_at"),
  }
}

/// Logs a failed statement under a fixed label and hands the route a generic error.
fn db_error(label: &'static str) -> impl FnOnce(tokio_postgres::Error) -> Error {
  move |e| {
    console_error!("{label}: {e}");
    Error::RustError("Internal Server Error".into())
  }
}

/// The flock a [`FlockAccess`] proved, as a uuid.
pub(crate) fn flock_uuid(access: &FlockAccess) -> Result<Uuid> {
  Uuid::parse_str(access.flock_id()).map_err(|_| Error::RustError("Invalid flock_id".into()))
}

/// The flock's rules, newest first.
pub async fn list_rules(client: &Client, access: &FlockAccess) -> Result<Vec<Rule>> {
  ensure_rule_tables(client).await?;
  let rows = client
    .query_typed(
      concat!(
        "SELECT ",
        rule_columns!(),
        " FROM rules r WHERE r.flock_id = $1 ORDER BY r.created_at DESC, r.id"
      ),
      &[(&flock_uuid(access)?, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule list error"))?;
  Ok(rows.iter().map(|r| Rule::from(rule_row(r))).collect())
}

/// Creates a disabled rule with no version. `recipients` is already normalized.
pub async fn create_rule(
  client: &Client,
  access: &FlockAccess,
  created_by: Uuid,
  name: &str,
  recipients: &[String],
) -> Result<Rule> {
  ensure_rule_tables(client).await?;
  let recipients =
    serde_json::to_string(recipients).map_err(|e| Error::RustError(e.to_string()))?;
  let row = client
    .query_typed_opt(
      concat!(
        "INSERT INTO rules AS r (flock_id, created_by, name, recipients)
         VALUES ($1, $2, $3, $4::jsonb) RETURNING ",
        rule_columns!()
      ),
      &[
        (&flock_uuid(access)?, Type::UUID),
        (&created_by, Type::UUID),
        (&name, Type::TEXT),
        (&recipients, Type::TEXT),
      ],
    )
    .await
    .map_err(db_error("Rule create error"))?
    .ok_or_else(|| Error::RustError("Rule create returned no row".into()))?;
  Ok(Rule::from(rule_row(&row)))
}

/// The rule and its versions, newest first; `None` when the flock has no such rule.
pub async fn rule_detail(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<Option<RuleDetail>> {
  ensure_rule_tables(client).await?;
  let Some(rule) = client
    .query_typed_opt(
      concat!(
        "SELECT ",
        rule_columns!(),
        " FROM rules r WHERE r.id = $1 AND r.flock_id = $2"
      ),
      &[(&rule_id, Type::UUID), (&flock_uuid(access)?, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule read error"))?
  else {
    return Ok(None);
  };
  let versions = client
    .query_typed(
      concat!(
        "SELECT ",
        version_columns!(),
        " FROM rule_versions v WHERE v.rule_id = $1 ORDER BY v.version DESC"
      ),
      &[(&rule_id, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule version list error"))?;
  Ok(Some(RuleDetail {
    rule: Rule::from(rule_row(&rule)),
    versions: versions
      .iter()
      .map(|v| RuleVersion::from(version_row(v)))
      .collect(),
  }))
}

/// Locks and reads the rule's row in `tx`; `None` when the flock has no such rule.
async fn lock_rule_row(
  tx: &Transaction<'_>,
  flock: Uuid,
  rule_id: Uuid,
) -> Result<Option<RuleRow>> {
  let row = tx
    .query_typed_opt(
      concat!(
        "SELECT ",
        rule_columns!(),
        ", now() AS read_at FROM rules r WHERE r.id = $1 AND r.flock_id = $2 FOR UPDATE"
      ),
      &[(&rule_id, Type::UUID), (&flock, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule lock error"))?;
  Ok(row.as_ref().map(rule_row))
}

/// How many rules of `flock` other than `rule_id` are enabled.
async fn enabled_siblings(tx: &Transaction<'_>, flock: Uuid, rule_id: Uuid) -> Result<i64> {
  let row = tx
    .query_typed_one(
      "SELECT count(*), now() FROM rules WHERE flock_id = $1 AND enabled AND id <> $2",
      &[(&flock, Type::UUID), (&rule_id, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule count error"))?;
  Ok(row.get(0))
}

/// What an update did.
#[derive(Debug)]
pub enum UpdateOutcome {
  /// The rule as it now stands.
  Updated(Rule),
  /// The flock has no such rule.
  NotFound,
  /// Nothing changed.
  Refused(EnableRefusal),
}

/// Renames, re-addresses, enables or disables a rule. Enabling clears the pause and the failure
/// streak; disabling a running rule records the owner as the reason. `recipients` is already
/// normalized.
pub async fn update_rule(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  name: Option<&str>,
  recipients: Option<&[String]>,
  enabled: Option<bool>,
) -> Result<UpdateOutcome> {
  ensure_rule_tables(client).await?;
  let flock = flock_uuid(access)?;
  let recipients = match recipients {
    Some(to) => Some(serde_json::to_string(to).map_err(|e| Error::RustError(e.to_string()))?),
    None => None,
  };
  let tx = begin(client)
    .await
    .map_err(db_error("Rule update begin error"))?;
  let body = async {
    if enabled == Some(true) {
      tx.query_typed(
        "SELECT 1 FROM flocks WHERE id = $1 FOR NO KEY UPDATE",
        &[(&flock, Type::UUID)],
      )
      .await
      .map_err(db_error("Flock lock error"))?;
    }
    let Some(current) = lock_rule_row(&tx, flock, rule_id).await? else {
      return Ok(UpdateOutcome::NotFound);
    };
    if enabled == Some(true) && !current.enabled {
      let siblings = enabled_siblings(&tx, flock, rule_id).await?;
      if let Some(refusal) = enable_refusal(&current, siblings) {
        return Ok(UpdateOutcome::Refused(refusal));
      }
    }
    let row = tx
      .query_typed_one(
        concat!(
          "UPDATE rules AS r SET
             name = COALESCE($3, r.name),
             recipients = COALESCE($4::jsonb, r.recipients),
             enabled = COALESCE($5, r.enabled),
             paused_reason = CASE WHEN $5 IS NULL OR $5 = r.enabled THEN r.paused_reason
                                  WHEN $5 THEN NULL ELSE 'owner' END,
             fail_count = CASE WHEN $5 AND NOT r.enabled THEN 0 ELSE r.fail_count END,
             fail_since = CASE WHEN $5 AND NOT r.enabled THEN NULL ELSE r.fail_since END,
             updated_at = now()
           WHERE r.id = $1 AND r.flock_id = $2
           RETURNING ",
          rule_columns!()
        ),
        &[
          (&rule_id, Type::UUID),
          (&flock, Type::UUID),
          (&name, Type::TEXT),
          (&recipients, Type::TEXT),
          (&enabled, Type::BOOL),
        ],
      )
      .await
      .map_err(db_error("Rule update error"))?;
    Ok(UpdateOutcome::Updated(Rule::from(rule_row(&row))))
  }
  .await;
  end(tx, body, db_error("Rule update commit error")).await
}

/// What a delete did.
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
  /// The rule and every row cascading from it are gone.
  Deleted,
  /// The flock has no such rule.
  NotFound,
  /// The rule has a module in the flock's script, or one uploading, so its delete must rebuild
  /// that script; nothing was changed.
  InScript,
}

/// Deletes a rule that never reached the flock's script, with every row that cascades from it.
/// One statement and no transaction: the `DELETE` rechecks its condition on the row it locks, so
/// a deploy marking the rule pending first leaves it standing, and the route that goes on to
/// rebuild the script reuses a client with nothing outstanding on it.
pub async fn delete_unscripted_rule(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<DeleteOutcome> {
  ensure_rule_tables(client).await?;
  let row = client
    .query_typed_one(
      "WITH gone AS (DELETE FROM rules WHERE id = $1 AND flock_id = $2
         AND deployed_version IS NULL AND pending_version IS NULL RETURNING 1)
       SELECT EXISTS (SELECT 1 FROM gone),
              EXISTS (SELECT 1 FROM rules WHERE id = $1 AND flock_id = $2)",
      &[(&rule_id, Type::UUID), (&flock_uuid(access)?, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule delete error"))?;
  Ok(match (row.get(0), row.get(1)) {
    (true, _) => DeleteOutcome::Deleted,
    (false, true) => DeleteOutcome::InScript,
    (false, false) => DeleteOutcome::NotFound,
  })
}

/// What a save did.
#[derive(Debug)]
pub enum SaveOutcome {
  /// The new version, with its source.
  Saved(Box<RuleVersion>),
  /// The flock has no such rule.
  NotFound,
  /// `based_on` names no version of this rule.
  NoBase,
}

/// Stores `version`, checked by `prepare_rule_version`, as the rule's next version, with the
/// builder's model when it made the version, then drops the oldest versions past
/// [`MAX_RULE_VERSIONS`] that are neither deployed nor uploading.
pub async fn save_version(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  created_by: Uuid,
  version: &PreparedRuleVersion,
  based_on: Option<i32>,
) -> Result<SaveOutcome> {
  ensure_rule_tables(client).await?;
  let flock = flock_uuid(access)?;
  let source = version.source.as_str();
  let builder_model = version.builder_model.as_deref();
  let sha256 = sha256_hex(source.as_bytes());
  let contract = RULE_CONTRACT as i32;
  let origin = version.origin.as_str();
  let tx = begin(client)
    .await
    .map_err(db_error("Rule save begin error"))?;
  let body = async {
    let Some(current) = lock_rule_row(&tx, flock, rule_id).await? else {
      return Ok(SaveOutcome::NotFound);
    };
    let Some(row) = tx
      .query_typed_opt(
        concat!(
          "INSERT INTO rule_versions AS v
             (rule_id, version, source, sha256, contract, origin, based_on, created_by,
              builder_model)
           SELECT $1, COALESCE(MAX(version), 0) + 1, $2, $3, $4, $5, $6, $7, $8::jsonb
           FROM rule_versions WHERE rule_id = $1
           HAVING $6::integer IS NULL OR bool_or(version = $6)
           RETURNING ",
          version_columns!(),
          ", v.test_summary::text AS test_summary, v.source,
          v.builder_model::text AS builder_model"
        ),
        &[
          (&rule_id, Type::UUID),
          (&source, Type::TEXT),
          (&sha256, Type::TEXT),
          (&contract, Type::INT4),
          (&origin, Type::TEXT),
          (&based_on, Type::INT4),
          (&created_by, Type::UUID),
          (&builder_model, Type::TEXT),
        ],
      )
      .await
      .map_err(db_error("Rule save error"))?
    else {
      return Ok(SaveOutcome::NoBase);
    };
    let keep: Vec<i32> = [current.deployed_version, current.pending_version]
      .into_iter()
      .flatten()
      .collect();
    tx.query_typed(
      "DELETE FROM rule_versions
       WHERE rule_id = $1 AND version <> ALL($3)
         AND version <= (SELECT version FROM rule_versions WHERE rule_id = $1
                         ORDER BY version DESC OFFSET $2 LIMIT 1)",
      &[
        (&rule_id, Type::UUID),
        (&MAX_RULE_VERSIONS, Type::INT8),
        (&keep, Type::INT4_ARRAY),
      ],
    )
    .await
    .map_err(db_error("Rule version prune error"))?;
    Ok(SaveOutcome::Saved(Box::new(RuleVersion::from(
      version_row(&row),
    ))))
  }
  .await;
  end(tx, body, db_error("Rule save commit error")).await
}

/// One version with its source and test summary; `None` when the flock has no such version.
pub async fn read_version(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
) -> Result<Option<RuleVersion>> {
  ensure_rule_tables(client).await?;
  let row = client
    .query_typed_opt(
      concat!(
        "SELECT ",
        version_columns!(),
        ", v.test_summary::text AS test_summary, v.source,
           v.builder_model::text AS builder_model
         FROM rule_versions v JOIN rules r ON r.id = v.rule_id
         WHERE v.rule_id = $1 AND r.flock_id = $2 AND v.version = $3"
      ),
      &[
        (&rule_id, Type::UUID),
        (&flock_uuid(access)?, Type::UUID),
        (&version, Type::INT4),
      ],
    )
    .await
    .map_err(db_error("Rule version read error"))?;
  Ok(row.as_ref().map(|v| RuleVersion::from(version_row(v))))
}

/// The rule's newest [`MAX_RULE_RUNS_PER_PAGE`] run records; `None` when the flock has no such
/// rule.
pub async fn list_runs(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<Option<Vec<RuleRun>>> {
  ensure_rule_tables(client).await?;
  let found = client
    .query_typed_opt(
      "SELECT 1 FROM rules WHERE id = $1 AND flock_id = $2",
      &[(&rule_id, Type::UUID), (&flock_uuid(access)?, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule read error"))?;
  if found.is_none() {
    return Ok(None);
  }
  let rows = client
    .query_typed(
      "SELECT u.rule_id, u.version, u.pigeon_id, u.batch_id, u.status, u.readings,
         u.alerts_sent, u.alerts_suppressed, u.values_written, u.state_changed, u.wall_ms,
         u.error, u.logs::text AS logs, u.ran_at
       FROM rule_runs u WHERE u.rule_id = $1 ORDER BY u.ran_at DESC LIMIT $2",
      &[
        (&rule_id, Type::UUID),
        (&MAX_RULE_RUNS_PER_PAGE, Type::INT8),
      ],
    )
    .await
    .map_err(db_error("Rule run list error"))?;
  Ok(Some(
    rows.iter().map(|r| RuleRun::from(run_row(r))).collect(),
  ))
}

/// Starts recording every run of the rule for an hour, or leaves a longer watch as it is;
/// `None` when the flock has no such rule.
pub async fn watch_rule(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<Option<Rule>> {
  ensure_rule_tables(client).await?;
  let row = client
    .query_typed_opt(
      concat!(
        "UPDATE rules AS r SET watch_until = GREATEST(r.watch_until, now() + interval '1 hour')
         WHERE r.id = $1 AND r.flock_id = $2
         RETURNING ",
        rule_columns!()
      ),
      &[(&rule_id, Type::UUID), (&flock_uuid(access)?, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule watch error"))?;
  Ok(row.as_ref().map(|r| Rule::from(rule_row(r))))
}

/// The rule's latest value for each pigeon and key, by pigeon then key, at most
/// [`MAX_RULE_VALUES_RETURNED`]; `None` when the flock has no such rule.
pub async fn latest_values(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<Option<Vec<RuleValue>>> {
  ensure_rule_tables(client).await?;
  let found = client
    .query_typed_opt(
      "SELECT 1 FROM rules WHERE id = $1 AND flock_id = $2",
      &[(&rule_id, Type::UUID), (&flock_uuid(access)?, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule read error"))?;
  if found.is_none() {
    return Ok(None);
  }
  let rows = client
    .query_typed(
      "SELECT DISTINCT ON (pigeon_id, key) pigeon_id, key, value, value_num, at
       FROM rule_values WHERE rule_id = $1
       ORDER BY pigeon_id, key, at DESC LIMIT $2",
      &[
        (&rule_id, Type::UUID),
        (&MAX_RULE_VALUES_RETURNED, Type::INT8),
      ],
    )
    .await
    .map_err(db_error("Rule value list error"))?;
  Ok(Some(
    rows
      .iter()
      .map(|r| {
        RuleValue::from(RuleValueRow {
          pigeon_id: r.get("pigeon_id"),
          key: r.get("key"),
          value: r.get("value"),
          value_num: r.get("value_num"),
          at: r.get("at"),
        })
      })
      .collect(),
  ))
}

/// A fresh read of what a test or a deploy of one version decides on.
#[derive(Debug, Clone)]
pub struct Release {
  pub rule: RuleRow,
  /// Without source or test summary.
  pub version: RuleVersionRow,
  /// Enabled rules of the flock other than this one.
  pub enabled_siblings: i64,
}

/// Reads the rule, the version and the flock's enabled count under the rule's lock, so a test
/// record written a moment ago is seen; `None` when the flock has no such version.
///
/// The enabled count is advisory: it is taken without the flock's row lock and committed at once,
/// so two deploys of two disabled rules can each see room for one more. The transaction that
/// promotes a deploy must lock the flock's row `FOR NO KEY UPDATE` and recount before it sets
/// `enabled`, as an enable does.
pub async fn read_release(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
) -> Result<Option<Release>> {
  ensure_rule_tables(client).await?;
  let flock = flock_uuid(access)?;
  let tx = begin(client)
    .await
    .map_err(db_error("Rule release begin error"))?;
  let body = async {
    let Some(rule) = lock_rule_row(&tx, flock, rule_id).await? else {
      return Ok(None);
    };
    let Some(version) = tx
      .query_typed_opt(
        concat!(
          "SELECT ",
          version_columns!(),
          ", now() AS read_at FROM rule_versions v WHERE v.rule_id = $1 AND v.version = $2"
        ),
        &[(&rule_id, Type::UUID), (&version, Type::INT4)],
      )
      .await
      .map_err(db_error("Rule release version error"))?
    else {
      return Ok(None);
    };
    let enabled_siblings = enabled_siblings(&tx, flock, rule_id).await?;
    Ok(Some(Release {
      rule,
      version: version_row(&version),
      enabled_siblings,
    }))
  }
  .await;
  end(tx, body, db_error("Rule release commit error")).await
}

/// One rule module of a flock's script, as the bundle is built from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleSource {
  /// The rule, whose uuid keys its module.
  pub rule_id: Uuid,
  /// The version the module holds.
  pub version: i32,
  /// That version's code.
  pub source: String,
}

/// What a flock's script holds: every unblocked rule at its deployed version, enabled or not,
/// the candidate rule (`$2`) at version `$3`, and never the rule being deleted (`$4`), oldest rule
/// first. A disabled rule keeps its module because an enable is a row change only: dropped from
/// the script, a re-enabled rule would have nothing to run.
const BUNDLE_SOURCES_SQL: &str = "SELECT r.id, v.version, v.source
  FROM rules r
  JOIN rule_versions v ON v.rule_id = r.id
   AND v.version = CASE WHEN r.id = $2 THEN $3 ELSE r.deployed_version END
  WHERE r.flock_id = $1 AND r.blocked_at IS NULL AND r.id <> $4
  ORDER BY r.created_at, r.id";

async fn bundle_sources(
  tx: &Transaction<'_>,
  flock: Uuid,
  candidate: Option<(Uuid, i32)>,
  leave_out: Option<Uuid>,
) -> Result<Vec<BundleSource>> {
  let (rule, version) = candidate.unwrap_or((Uuid::nil(), 0));
  let leave_out = leave_out.unwrap_or(Uuid::nil());
  let rows = tx
    .query_typed(
      BUNDLE_SOURCES_SQL,
      &[
        (&flock, Type::UUID),
        (&rule, Type::UUID),
        (&version, Type::INT4),
        (&leave_out, Type::UUID),
      ],
    )
    .await
    .map_err(db_error("Rule bundle read error"))?;
  Ok(
    rows
      .iter()
      .map(|row| BundleSource {
        rule_id: row.get(0),
        version: row.get(1),
        source: row.get(2),
      })
      .collect(),
  )
}

/// What a dry run reads beside its release.
#[derive(Debug, Clone, PartialEq)]
pub struct DryRunSources {
  /// The tested version's code.
  pub candidate: String,
  /// The deployed version and its code, when it is another version than the tested one.
  pub deployed: Option<(i32, String)>,
  /// Every enabled, unblocked rule of the flock at its deployed version, and this rule at the
  /// tested version in its own place, oldest rule first: what the flock would dispatch once the
  /// version is deployed.
  pub bundle: Vec<BundleSource>,
  /// The live state of each bundle rule, for the given pigeons and the flock.
  pub seeds: HashMap<Uuid, Seed>,
}

const DRY_RUN_BUNDLE_SQL: &str = "SELECT r.id, v.version, v.source
  FROM rules r
  JOIN rule_versions v ON v.rule_id = r.id
   AND v.version = CASE WHEN r.id = $2 THEN $3 ELSE r.deployed_version END
  WHERE r.flock_id = $1 AND r.blocked_at IS NULL AND (r.id = $2 OR r.enabled)
  ORDER BY r.created_at, r.id";

/// Reads what a dry run of `version` needs; `None` when the version is gone. `pigeons` are the
/// replay's, so no state is read under any other pigeon.
pub async fn dry_run_sources(
  client: &Client,
  access: &FlockAccess,
  release: &Release,
  pigeons: &[String],
) -> Result<Option<DryRunSources>> {
  let flock = flock_uuid(access)?;
  let rule_id = release.rule.id;
  let version = release.version.version;
  let rows = client
    .query_typed(
      DRY_RUN_BUNDLE_SQL,
      &[
        (&flock, Type::UUID),
        (&rule_id, Type::UUID),
        (&version, Type::INT4),
      ],
    )
    .await
    .map_err(db_error("Rule dry-run bundle read error"))?;
  let bundle: Vec<BundleSource> = rows
    .iter()
    .map(|row| BundleSource {
      rule_id: row.get(0),
      version: row.get(1),
      source: row.get(2),
    })
    .collect();
  let Some(candidate) = bundle
    .iter()
    .find(|s| s.rule_id == rule_id)
    .map(|s| s.source.clone())
  else {
    return Ok(None);
  };
  let deployed = match release.rule.deployed_version.filter(|d| *d != version) {
    Some(deployed) => client
      .query_typed_opt(
        "SELECT source FROM rule_versions WHERE rule_id = $1 AND version = $2",
        &[(&rule_id, Type::UUID), (&deployed, Type::INT4)],
      )
      .await
      .map_err(db_error("Rule dry-run deployed read error"))?
      .map(|row| (deployed, row.get(0))),
    None => None,
  };
  let rules: Vec<Uuid> = bundle.iter().map(|s| s.rule_id).collect();
  let rows = client
    .query_typed(
      "SELECT rule_id, scope, doc::text FROM rule_state
       WHERE rule_id = ANY($1) AND (scope = '' OR scope = ANY($2))",
      &[(&rules, Type::UUID_ARRAY), (&pigeons, Type::TEXT_ARRAY)],
    )
    .await
    .map_err(db_error("Rule dry-run state read error"))?;
  let mut seeds: HashMap<Uuid, Seed> = HashMap::new();
  for row in &rows {
    let text: String = row.get(2);
    let Ok(serde_json::Value::Object(doc)) = serde_json::from_str(&text) else {
      continue;
    };
    let seed = seeds.entry(row.get(0)).or_default();
    let scope: String = row.get(1);
    if scope.is_empty() {
      seed.flock = doc;
    } else {
      seed.pigeons.insert(scope, doc);
    }
  }
  Ok(Some(DryRunSources {
    candidate,
    deployed,
    bundle,
    seeds,
  }))
}

/// Records a finished dry run on the version it tested, only while the version still holds the
/// bytes that ran, and answers it as the single read does; `None` when it no longer does.
pub async fn record_test(
  client: &Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
  sha256: &str,
  summary: &str,
) -> Result<Option<RuleVersion>> {
  let contract = RULE_CONTRACT as i32;
  let row = client
    .query_typed_opt(
      concat!(
        "UPDATE rule_versions v SET tested_sha256 = v.sha256, tested_contract = $4,
           tested_at = now(), test_summary = $5::jsonb
         FROM rules r
         WHERE r.id = v.rule_id AND r.flock_id = $1 AND v.rule_id = $2 AND v.version = $3
           AND v.sha256 = $6
         RETURNING ",
        version_columns!(),
        ", v.test_summary::text AS test_summary, v.source,
          v.builder_model::text AS builder_model"
      ),
      &[
        (&flock_uuid(access)?, Type::UUID),
        (&rule_id, Type::UUID),
        (&version, Type::INT4),
        (&contract, Type::INT4),
        (&summary, Type::TEXT),
        (&sha256, Type::TEXT),
      ],
    )
    .await
    .map_err(db_error("Rule test record error"))?;
  Ok(row.as_ref().map(|v| RuleVersion::from(version_row(v))))
}

/// Frees a lease after a failure, logging rather than failing: an unfreed lease lapses by itself.
pub(crate) async fn free_lease(client: &Client, flock: Uuid, lease: OffsetDateTime) {
  if let Err(e) = release_deploy_lease(client, flock, lease).await {
    console_error!("Rule lease release error: {e}");
  }
}

/// How a deploy started.
#[derive(Debug)]
pub enum DeployStart {
  /// Another deploy holds the flock's lease: the route's 409.
  Busy,
  /// The flock has no such rule.
  Gone,
  /// The lease is held and `pending_version` set; `sources` is the script to upload.
  Started {
    lease: OffsetDateTime,
    sources: Vec<BundleSource>,
  },
}

/// Takes the flock's deploy lease, marks `version` pending on the rule and reads the script to
/// upload. The caller ends it with [`finish_deploy`] or [`abandon_deploy`].
pub async fn start_deploy(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
  account: Uuid,
) -> Result<DeployStart> {
  ensure_rule_tables(client).await?;
  let flock = flock_uuid(access)?;
  let tag = account_tag(&uuid_str(account));
  let Some(lease) = take_deploy_lease(client, flock, RULE_CONTRACT as i32, &tag)
    .await
    .map_err(db_error("Rule lease error"))?
  else {
    return Ok(DeployStart::Busy);
  };
  let started = match begin(client).await {
    Ok(tx) => {
      let body = async {
        if lock_rule_row(&tx, flock, rule_id).await?.is_none() {
          return Ok(None);
        }
        tx.query_typed(
          "UPDATE rules SET pending_version = $2 WHERE id = $1",
          &[(&rule_id, Type::UUID), (&version, Type::INT4)],
        )
        .await
        .map_err(db_error("Rule pending error"))?;
        bundle_sources(&tx, flock, Some((rule_id, version)), None)
          .await
          .map(Some)
      }
      .await;
      end(tx, body, db_error("Rule deploy commit error")).await
    }
    Err(e) => Err(db_error("Rule deploy begin error")(e)),
  };
  match started {
    Ok(Some(sources)) => Ok(DeployStart::Started { lease, sources }),
    Ok(None) => {
      free_lease(client, flock, lease).await;
      Ok(DeployStart::Gone)
    }
    Err(e) => {
      free_lease(client, flock, lease).await;
      Err(e)
    }
  }
}

/// How a deploy ended after its upload succeeded.
#[derive(Debug)]
pub enum DeployFinish {
  /// The rule as promoted. It stays disabled when the flock filled while the upload ran.
  Promoted(Rule),
  /// The lease lapsed and another deploy took it, so its script may be newer than this one;
  /// nothing was promoted.
  LeaseLost,
}

/// Locks the flock's bundle row while it still holds `lease`; false when it does not.
async fn lock_lease(tx: &Transaction<'_>, flock: Uuid, lease: OffsetDateTime) -> Result<bool> {
  let held = tx
    .query_typed_opt(
      "SELECT now() FROM rule_bundles WHERE flock_id = $1 AND deploy_until = $2 FOR UPDATE",
      &[(&flock, Type::UUID), (&lease, Type::TIMESTAMPTZ)],
    )
    .await
    .map_err(db_error("Rule lease lock error"))?;
  Ok(held.is_some())
}

/// Promotes `version` after its upload, in one transaction: deployed, no longer pending, enabled
/// when the flock has room, watched for 24 hours, its state reset when asked, the bundle row
/// recorded and the lease freed. A sibling's pending version is cleared too: the upload built that
/// rule at its deployed version, undoing whatever an unconfirmed deploy of it left. Locks the
/// flock, then the bundle row, then the rule, so the enabled count it decides on is the one it
/// commits.
pub async fn finish_deploy(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
  lease: OffsetDateTime,
  reset_state: bool,
  account: Uuid,
) -> Result<DeployFinish> {
  let flock = flock_uuid(access)?;
  let tag = account_tag(&uuid_str(account));
  let contract = RULE_CONTRACT as i32;
  let tx = begin(client)
    .await
    .map_err(db_error("Rule promote begin error"))?;
  let body = async {
    tx.query_typed(
      "SELECT 1 FROM flocks WHERE id = $1 FOR NO KEY UPDATE",
      &[(&flock, Type::UUID)],
    )
    .await
    .map_err(db_error("Flock lock error"))?;
    if !lock_lease(&tx, flock, lease).await? {
      tx.query_typed(
        "UPDATE rules SET pending_version = NULL WHERE id = $1 AND pending_version = $2",
        &[(&rule_id, Type::UUID), (&version, Type::INT4)],
      )
      .await
      .map_err(db_error("Rule pending error"))?;
      return Ok(DeployFinish::LeaseLost);
    }
    let Some(current) = lock_rule_row(&tx, flock, rule_id).await? else {
      // Unreachable while deletes take the lease too; frees it rather than wait out its 60 s.
      tx.query_typed(
        "UPDATE rule_bundles SET deploy_until = 'epoch' WHERE flock_id = $1",
        &[(&flock, Type::UUID)],
      )
      .await
      .map_err(db_error("Rule lease release error"))?;
      return Ok(DeployFinish::LeaseLost);
    };
    let enable = current.enabled
      || (current.blocked_at.is_none()
        && enabled_siblings(&tx, flock, rule_id).await? < MAX_ENABLED_RULES_PER_FLOCK);
    let row = tx
      .query_typed_one(
        concat!(
          "UPDATE rules AS r SET
             deployed_version = $2,
             pending_version = NULL,
             enabled = $3,
             paused_reason = CASE WHEN $3 THEN NULL ELSE r.paused_reason END,
             fail_count = CASE WHEN $3 AND NOT r.enabled THEN 0 ELSE r.fail_count END,
             fail_since = CASE WHEN $3 AND NOT r.enabled THEN NULL ELSE r.fail_since END,
             watch_until = now() + interval '24 hours',
             updated_at = now()
           WHERE r.id = $1
           RETURNING ",
          rule_columns!()
        ),
        &[
          (&rule_id, Type::UUID),
          (&version, Type::INT4),
          (&enable, Type::BOOL),
        ],
      )
      .await
      .map_err(db_error("Rule promote error"))?;
    if reset_state {
      tx.query_typed(
        "DELETE FROM rule_state WHERE rule_id = $1",
        &[(&rule_id, Type::UUID)],
      )
      .await
      .map_err(db_error("Rule state reset error"))?;
    }
    tx.query_typed(
      "UPDATE rules SET pending_version = NULL
       WHERE flock_id = $1 AND id <> $2 AND pending_version IS NOT NULL",
      &[(&flock, Type::UUID), (&rule_id, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule pending error"))?;
    tx.query_typed(
      "UPDATE rule_bundles SET contract = $2, account_tag = $3, uploaded_at = now(),
         deploy_until = 'epoch'
       WHERE flock_id = $1",
      &[
        (&flock, Type::UUID),
        (&contract, Type::INT4),
        (&tag, Type::TEXT),
      ],
    )
    .await
    .map_err(db_error("Rule bundle record error"))?;
    Ok(DeployFinish::Promoted(Rule::from(rule_row(&row))))
  }
  .await;
  end(tx, body, db_error("Rule promote commit error")).await
}

/// Ends a deploy whose upload was refused: clears `pending_version` and frees the lease. An upload
/// that may have landed is not abandoned: its lease is left to lapse, so no other upload starts
/// while the aborted one may still finish, and the version stays pending for a retry or the sweep.
pub async fn abandon_deploy(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
  lease: OffsetDateTime,
) -> Result<()> {
  let flock = flock_uuid(access)?;
  client
    .query_typed(
      "UPDATE rules SET pending_version = NULL
       WHERE id = $1 AND flock_id = $2 AND pending_version = $3",
      &[
        (&rule_id, Type::UUID),
        (&flock, Type::UUID),
        (&version, Type::INT4),
      ],
    )
    .await
    .map_err(db_error("Rule pending error"))?;
  release_deploy_lease(client, flock, lease)
    .await
    .map_err(db_error("Rule lease release error"))?;
  Ok(())
}

/// How a rebuild of a flock's script started.
#[derive(Debug)]
pub enum RebuildStart {
  /// Another deploy holds the flock's lease.
  Busy,
  /// The flock has no rule to disable.
  NotFound,
  /// The lease is held; `sources` is the script to upload, empty when no module is left.
  Started {
    lease: OffsetDateTime,
    sources: Vec<BundleSource>,
  },
}

/// Takes the lease and reads `flock`'s script as its rows now make it. `disable` names a rule
/// being deleted: it is disabled first, in this transaction, so no batch dispatches to it while
/// the script changes, and the rebuild leaves it out. Takes the flock's uuid rather than a
/// [`FlockAccess`] because the daily sweep rebuilds too; a route passes the flock it proved.
pub async fn start_rebuild(
  client: &mut Client,
  flock: Uuid,
  disable: Option<Uuid>,
  account: Uuid,
) -> Result<RebuildStart> {
  ensure_rule_tables(client).await?;
  let tag = account_tag(&uuid_str(account));
  let Some(lease) = take_deploy_lease(client, flock, RULE_CONTRACT as i32, &tag)
    .await
    .map_err(db_error("Rule lease error"))?
  else {
    return Ok(RebuildStart::Busy);
  };
  let started = match begin(client).await {
    Ok(tx) => {
      let body = async {
        if let Some(rule_id) = disable {
          if lock_rule_row(&tx, flock, rule_id).await?.is_none() {
            return Ok(None);
          }
          tx.query_typed(
            "UPDATE rules SET enabled = false, updated_at = now() WHERE id = $1",
            &[(&rule_id, Type::UUID)],
          )
          .await
          .map_err(db_error("Rule disable error"))?;
        }
        bundle_sources(&tx, flock, None, disable).await.map(Some)
      }
      .await;
      end(tx, body, db_error("Rule rebuild commit error")).await
    }
    Err(e) => Err(db_error("Rule rebuild begin error")(e)),
  };
  match started {
    Ok(Some(sources)) => Ok(RebuildStart::Started { lease, sources }),
    Ok(None) => {
      free_lease(client, flock, lease).await;
      Ok(RebuildStart::NotFound)
    }
    Err(e) => {
      free_lease(client, flock, lease).await;
      Err(e)
    }
  }
}

/// Records a rebuild that reached the namespace, while the lease is still held: deletes the rule
/// `delete` names, clears any pending version left in the flock (no live deploy can own one
/// while the lease is held), and records the bundle, or removes its row when `script_gone`.
/// False, with nothing changed, when the lease lapsed.
pub async fn finish_rebuild(
  client: &mut Client,
  flock: Uuid,
  lease: OffsetDateTime,
  delete: Option<Uuid>,
  script_gone: bool,
  account: Uuid,
) -> Result<bool> {
  let tag = account_tag(&uuid_str(account));
  let contract = RULE_CONTRACT as i32;
  let tx = begin(client)
    .await
    .map_err(db_error("Rule rebuild begin error"))?;
  let body = async {
    if !lock_lease(&tx, flock, lease).await? {
      return Ok(false);
    }
    if let Some(rule_id) = delete {
      tx.query_typed(
        "DELETE FROM rules WHERE id = $1 AND flock_id = $2",
        &[(&rule_id, Type::UUID), (&flock, Type::UUID)],
      )
      .await
      .map_err(db_error("Rule delete error"))?;
    }
    tx.query_typed(
      "UPDATE rules SET pending_version = NULL
       WHERE flock_id = $1 AND pending_version IS NOT NULL",
      &[(&flock, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule pending error"))?;
    if script_gone {
      tx.query_typed(
        "DELETE FROM rule_bundles WHERE flock_id = $1",
        &[(&flock, Type::UUID)],
      )
      .await
      .map_err(db_error("Rule bundle delete error"))?;
    } else {
      tx.query_typed(
        "UPDATE rule_bundles SET contract = $2, account_tag = $3, uploaded_at = now(),
           deploy_until = 'epoch'
         WHERE flock_id = $1",
        &[
          (&flock, Type::UUID),
          (&contract, Type::INT4),
          (&tag, Type::TEXT),
        ],
      )
      .await
      .map_err(db_error("Rule bundle record error"))?;
    }
    Ok(true)
  }
  .await;
  end(tx, body, db_error("Rule rebuild commit error")).await
}

/// Every flock's bundle row with what the sweep decides on. A whole-table read, once a day.
const SWEEP_BUNDLES_SQL: &str = "SELECT b.flock_id, b.uploaded_at IS NOT NULL, b.account_tag,
    COALESCE(f.org_id, f.user_id), b.deploy_until > now(),
    b.deploy_until < now() - interval '10 minutes'
      AND EXISTS (SELECT 1 FROM rules r
                  WHERE r.flock_id = b.flock_id AND r.pending_version IS NOT NULL)
  FROM rule_bundles b JOIN flocks f ON f.id = b.flock_id";

/// The bundle rows for [`crate::helpers::rule_namespace::plan_sweep`]. A pending version counts
/// as stale once the lease has been free for ten minutes, so a deploy in flight, or an unconfirmed
/// one whose lease is left to lapse, is never swept.
pub async fn sweep_bundles(client: &Client) -> Result<Vec<SweepBundle>> {
  ensure_rule_tables(client).await?;
  let rows = client
    .query_typed(SWEEP_BUNDLES_SQL, &[])
    .await
    .map_err(db_error("Rule sweep read error"))?;
  Ok(
    rows
      .iter()
      .map(|row| SweepBundle {
        flock_id: row.get(0),
        uploaded: row.get(1),
        account_tag: row.get(2),
        owner: row.get(3),
        leased: row.get(4),
        stale_pending: row.get(5),
      })
      .collect(),
  )
}

/// Records that the flock's script is now tagged for `account`.
pub async fn retag_bundle(client: &Client, flock: Uuid, account: Uuid) -> Result<()> {
  let tag = account_tag(&uuid_str(account));
  client
    .query_typed(
      "UPDATE rule_bundles SET account_tag = $2 WHERE flock_id = $1",
      &[(&flock, Type::UUID), (&tag, Type::TEXT)],
    )
    .await
    .map_err(db_error("Rule retag error"))?;
  Ok(())
}

/// The owning organization or user of a flock, which its script is tagged for.
pub async fn flock_owner(client: &Client, flock: Uuid) -> Result<Option<Uuid>> {
  let row = client
    .query_typed_opt(
      "SELECT COALESCE(org_id, user_id), now() AS read_at FROM flocks WHERE id = $1",
      &[(&flock, Type::UUID)],
    )
    .await
    .map_err(db_error("Flock owner read error"))?;
  Ok(row.map(|row| row.get(0)))
}

/// Whether the flock has a script in the namespace, read before a flock delete cascades the row
/// away.
pub async fn bundle_uploaded(client: &Client, flock: Uuid) -> Result<bool> {
  ensure_rule_tables(client).await?;
  let row = client
    .query_typed_opt(
      "SELECT uploaded_at IS NOT NULL, now() AS read_at FROM rule_bundles WHERE flock_id = $1",
      &[(&flock, Type::UUID)],
    )
    .await
    .map_err(db_error("Rule bundle read error"))?;
  Ok(row.is_some_and(|row| row.get(0)))
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::OffsetDateTime;

  fn rule(enabled: bool, deployed: Option<i32>, blocked: bool) -> RuleRow {
    let now = OffsetDateTime::UNIX_EPOCH;
    RuleRow {
      id: Uuid::nil(),
      flock_id: Uuid::nil(),
      created_by: Uuid::nil(),
      name: "r".into(),
      recipients: "[]".into(),
      enabled,
      deployed_version: deployed,
      pending_version: None,
      paused_reason: None,
      blocked_at: blocked.then_some(now),
      fail_count: 0,
      watch_until: None,
      created_at: now,
      updated_at: now,
    }
  }

  fn version(tested_sha: Option<&str>, tested_contract: Option<i32>) -> RuleVersionRow {
    let now = OffsetDateTime::UNIX_EPOCH;
    RuleVersionRow {
      rule_id: Uuid::nil(),
      version: 2,
      sha256: "ab".into(),
      contract: 1,
      origin: "hand".into(),
      based_on: None,
      repair_depth: 0,
      prompt: None,
      tested_sha256: tested_sha.map(str::to_string),
      tested_contract,
      tested_at: tested_sha.map(|_| now),
      test_summary: None,
      created_by: Uuid::nil(),
      created_at: now,
      source: None,
      builder_model: None,
    }
  }

  #[test]
  fn enabling_needs_a_deploy_no_block_and_room_in_the_flock() {
    let full = MAX_ENABLED_RULES_PER_FLOCK;
    assert_eq!(
      enable_refusal(&rule(false, None, false), 0),
      Some(EnableRefusal::NotDeployed)
    );
    assert_eq!(
      enable_refusal(&rule(false, Some(1), true), 0),
      Some(EnableRefusal::Blocked)
    );
    assert_eq!(
      enable_refusal(&rule(false, Some(1), false), full),
      Some(EnableRefusal::FlockFull)
    );
    assert_eq!(enable_refusal(&rule(false, Some(1), false), full - 1), None);
    assert_eq!(enable_refusal(&rule(true, Some(1), false), full), None);
  }

  #[test]
  fn a_deploy_needs_a_test_of_these_bytes_under_this_harness() {
    let release = |rule, version, enabled_siblings| Release {
      rule,
      version,
      enabled_siblings,
    };
    let contract = RULE_CONTRACT as i32;
    let ok = version(Some("ab"), Some(contract));
    assert_eq!(
      deploy_refusal(&release(rule(false, None, false), ok.clone(), 0)),
      None
    );
    for untested in [
      version(None, None),
      version(Some("cd"), Some(contract)),
      version(Some("ab"), Some(contract + 1)),
    ] {
      assert_eq!(
        deploy_refusal(&release(rule(false, None, false), untested, 0)),
        Some(DeployRefusal::Untested)
      );
    }
    assert_eq!(
      deploy_refusal(&release(rule(false, None, true), ok.clone(), 0)),
      Some(DeployRefusal::Blocked)
    );
    let full = MAX_ENABLED_RULES_PER_FLOCK;
    assert_eq!(
      deploy_refusal(&release(rule(false, Some(1), false), ok.clone(), full)),
      Some(DeployRefusal::FlockFull)
    );
    assert_eq!(
      deploy_refusal(&release(rule(true, Some(1), false), ok, full)),
      None
    );
  }

  #[test]
  fn a_blocked_rule_is_refused_a_test_too() {
    let release = |rule| Release {
      rule,
      version: version(None, None),
      enabled_siblings: MAX_ENABLED_RULES_PER_FLOCK,
    };
    assert_eq!(
      test_refusal(&release(rule(false, None, true))),
      Some(DeployRefusal::Blocked)
    );
    assert_eq!(test_refusal(&release(rule(true, Some(1), false))), None);
  }

  #[test]
  fn rules_open_only_to_listed_organizations() {
    let org = Uuid::from_u128(0xb30f_82fd);
    let listed = org.to_string();
    let mut two = String::with_capacity(2 * listed.len() + 2);
    two.push_str("not-a-uuid, ");
    two.push_str(&listed);
    assert!(rules_org_allowed(Some(&listed), Some(org)));
    assert!(rules_org_allowed(Some(&two), Some(org)));
    assert!(!rules_org_allowed(None, Some(org)));
    assert!(!rules_org_allowed(Some(""), Some(org)));
    assert!(!rules_org_allowed(Some("not-a-uuid"), Some(org)));
    assert!(!rules_org_allowed(Some(&listed), Some(Uuid::nil())));
    assert!(!rules_org_allowed(Some(&listed), None));
  }

  #[test]
  fn the_flock_ceiling_text_names_its_cap() {
    assert!(FLOCK_FULL.contains(&MAX_ENABLED_RULES_PER_FLOCK.to_string()));
  }

  #[test]
  fn refusals_are_never_401() {
    let statuses = [
      EnableRefusal::NotDeployed.response().0,
      EnableRefusal::Blocked.response().0,
      EnableRefusal::FlockFull.response().0,
      DeployRefusal::Blocked.response().0,
      DeployRefusal::Untested.response().0,
      DeployRefusal::FlockFull.response().0,
      RULES_ORG_REFUSED.0,
    ];
    assert!(statuses.iter().all(|s| matches!(s, 403 | 409)));
  }
}

/// The statements against the dev Postgres, as [`super::rule_store`]'s `pg` tests run them.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod pg {
  use super::*;
  use crate::helpers::rule_store::pg::{
    Scratch, new_flock, new_rule, pid, pipeline_watched, run, wait_until_blocked,
  };
  use capsules::RuleOrigin;
  use std::sync::atomic::Ordering;
  use time::OffsetDateTime;

  fn access(flock: Uuid) -> FlockAccess {
    FlockAccess::assert_checked(&flock.to_string(), None)
  }

  /// A code version as the route prepares one.
  fn code(source: &str, origin: RuleOrigin) -> PreparedRuleVersion {
    PreparedRuleVersion {
      source: source.to_string(),
      origin,
      builder_model: None,
    }
  }

  async fn deploy(client: &Client, rule: Uuid, version: i32) {
    client
      .execute(
        "UPDATE rules SET deployed_version = $2 WHERE id = $1",
        &[&rule, &version],
      )
      .await
      .expect("deployed");
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_create_list_and_read_stay_inside_the_flock() {
    run(async {
      let db = Scratch::new("admin_read").await;
      let client = db.client().await;
      let flock = new_flock(&client, None).await;
      let other = new_flock(&client, None).await;
      let author = Uuid::from_u128(7);
      let to = vec!["ops@example.com".to_string()];
      let made = create_rule(&client, &access(flock), author, "Boots", &to)
        .await
        .expect("create");
      assert!(!made.enabled && made.deployed_version.is_none());
      assert_eq!((made.created_by, made.recipients.clone()), (author, to));

      let listed = list_rules(&client, &access(flock)).await.expect("list");
      assert_eq!(listed, std::slice::from_ref(&made));
      assert!(
        list_rules(&client, &access(other))
          .await
          .expect("list")
          .is_empty()
      );
      assert!(
        rule_detail(&client, &access(other), made.id)
          .await
          .expect("read")
          .is_none()
      );
      assert!(
        list_runs(&client, &access(other), made.id)
          .await
          .expect("runs")
          .is_none()
      );

      client
        .execute(
          "INSERT INTO pigeons (id, flock_id, connector, created_at, updated_at)
           VALUES ('p1', $1, '{}', now(), now())",
          &[&flock],
        )
        .await
        .expect("pigeon");
      for (batch, at) in [("b1", 100.0), ("b2", 200.0)] {
        client
          .execute(
            "INSERT INTO rule_runs (rule_id, version, pigeon_id, batch_id, status, logs, ran_at)
             VALUES ($1, 1, 'p1', $2, 'threw', '[\"line\"]', to_timestamp($3))",
            &[&made.id, &batch, &at],
          )
          .await
          .expect("run");
      }
      let runs = list_runs(&client, &access(flock), made.id)
        .await
        .expect("runs")
        .expect("rule found");
      assert_eq!(
        runs.iter().map(|r| r.batch_id.as_str()).collect::<Vec<_>>(),
        ["b2", "b1"]
      );
      assert_eq!(runs[0].logs, ["line"]);
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_versions_number_up_keep_fifty_and_spare_the_deployed() {
    run(async {
      let db = Scratch::new("admin_versions").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let rule = new_rule(&client, flock).await;
      let author = Uuid::from_u128(7);
      let a = access(flock);

      let SaveOutcome::Saved(first) = save_version(
        &mut client,
        &a,
        rule,
        author,
        &code("export default () => ({});", RuleOrigin::Hand),
        None,
      )
      .await
      .expect("save") else {
        panic!("first save refused");
      };
      assert_eq!(first.version, 1);
      assert_eq!(first.sha256, sha256_hex(b"export default () => ({});"));
      assert_eq!(first.contract, RULE_CONTRACT as i32);
      assert_eq!(first.source.as_deref(), Some("export default () => ({});"));
      assert!(matches!(
        save_version(
          &mut client,
          &a,
          rule,
          author,
          &code("x", RuleOrigin::Hand),
          Some(9),
        )
        .await
        .expect("save"),
        SaveOutcome::NoBase
      ));
      let elsewhere = access(new_flock(&client, None).await);
      assert!(matches!(
        save_version(
          &mut client,
          &elsewhere,
          rule,
          author,
          &code("x", RuleOrigin::Hand),
          None,
        )
        .await
        .expect("save"),
        SaveOutcome::NotFound
      ));
      deploy(&client, rule, 1).await;

      for n in 2..=60 {
        let SaveOutcome::Saved(saved) = save_version(
          &mut client,
          &a,
          rule,
          author,
          &code("x", RuleOrigin::AssistantEdited),
          Some(n - 1),
        )
        .await
        .expect("save") else {
          panic!("save refused");
        };
        assert_eq!(saved.version, n);
      }
      let kept: Vec<i32> = client
        .query(
          "SELECT version FROM rule_versions WHERE rule_id = $1 ORDER BY version",
          &[&rule],
        )
        .await
        .expect("versions")
        .iter()
        .map(|r| r.get(0))
        .collect();
      let mut expected = vec![1];
      expected.extend(11..=60);
      assert_eq!(kept, expected);

      let read = read_version(&client, &a, rule, 1)
        .await
        .expect("read")
        .expect("deployed version kept");
      assert_eq!(read.source.as_deref(), Some("export default () => ({});"));
      assert!(
        read_version(&client, &elsewhere, rule, 1)
          .await
          .expect("read")
          .is_none()
      );
      let detail = rule_detail(&client, &a, rule)
        .await
        .expect("detail")
        .expect("found");
      assert_eq!(detail.versions.first().map(|v| v.version), Some(60));
      assert!(detail.versions.iter().all(|v| v.source.is_none()));
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_builder_version_keeps_its_model_beside_its_module() {
    run(async {
      let db = Scratch::new("admin_builder").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let rule = new_rule(&client, flock).await;
      let a = access(flock);
      let model: capsules::RuleModel = serde_json::from_str(
        r#"{"v": 1, "when": {"telemetry": {"key": "temp_c"}},
          "then": [{"do": "mail", "severity": "info", "subject": "Seen {temp_c}"}]}"#,
      )
      .expect("a model");
      let prepared = capsules::prepare_rule_version(&capsules::RuleVersionCreateRequest {
        builder_model: Some(model.clone()),
        ..Default::default()
      })
      .expect("accepted");
      let SaveOutcome::Saved(saved) =
        save_version(&mut client, &a, rule, Uuid::from_u128(7), &prepared, None)
          .await
          .expect("save")
      else {
        panic!("save refused");
      };
      assert_eq!(saved.origin, RuleOrigin::Builder);
      assert_eq!(saved.builder_model.as_ref(), Some(&model));
      assert_eq!(saved.source.as_deref(), Some(prepared.source.as_str()));
      let read = read_version(&client, &a, rule, saved.version)
        .await
        .expect("read")
        .expect("found");
      assert_eq!(read.builder_model, Some(model));
      let detail = rule_detail(&client, &a, rule)
        .await
        .expect("detail")
        .expect("found");
      assert!(detail.versions.iter().all(|v| v.builder_model.is_none()));
      assert_eq!(detail.versions[0].origin, RuleOrigin::Builder);
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_update_enables_only_what_can_run() {
    run(async {
      let db = Scratch::new("admin_update").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let rule = new_rule(&client, flock).await;
      let a = access(flock);

      assert!(matches!(
        update_rule(&mut client, &a, rule, None, None, Some(true))
          .await
          .expect("update"),
        UpdateOutcome::Refused(EnableRefusal::NotDeployed)
      ));
      deploy(&client, rule, 1).await;
      client
        .execute(
          "UPDATE rules SET paused_reason = 'auto:threw', fail_count = 20, fail_since = now()
           WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("paused");
      let to = vec!["a@example.com".to_string()];
      let UpdateOutcome::Updated(on) = update_rule(
        &mut client,
        &a,
        rule,
        Some("Renamed"),
        Some(&to),
        Some(true),
      )
      .await
      .expect("update") else {
        panic!("enable refused");
      };
      assert!(on.enabled && on.paused_reason.is_none() && on.fail_count == 0);
      assert_eq!((on.name.as_str(), on.recipients.clone()), ("Renamed", to));

      let UpdateOutcome::Updated(off) = update_rule(&mut client, &a, rule, None, None, Some(false))
        .await
        .expect("update")
      else {
        panic!("disable refused");
      };
      assert!(!off.enabled);
      assert_eq!(off.paused_reason.as_deref(), Some("owner"));
      assert_eq!(off.name, "Renamed");

      client
        .execute(
          "UPDATE rules SET blocked_at = now() WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("blocked");
      assert!(matches!(
        update_rule(&mut client, &a, rule, None, None, Some(true))
          .await
          .expect("update"),
        UpdateOutcome::Refused(EnableRefusal::Blocked)
      ));
      let elsewhere = access(new_flock(&client, None).await);
      assert!(matches!(
        update_rule(&mut client, &elsewhere, rule, Some("x"), None, None)
          .await
          .expect("update"),
        UpdateOutcome::NotFound
      ));
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_racing_enable_recounts_behind_the_flock_lock() {
    run(async {
      let db = Scratch::new("admin_race").await;
      let setup = db.client().await;
      let flock = new_flock(&setup, None).await;
      for _ in 0..MAX_ENABLED_RULES_PER_FLOCK - 1 {
        let on = new_rule(&setup, flock).await;
        setup
          .execute(
            "UPDATE rules SET enabled = true, deployed_version = 1 WHERE id = $1",
            &[&on],
          )
          .await
          .expect("enabled");
      }
      let mine = new_rule(&setup, flock).await;
      let theirs = new_rule(&setup, flock).await;
      deploy(&setup, mine, 1).await;
      deploy(&setup, theirs, 1).await;

      let a = db.client().await;
      let mut b = db.client().await;
      let b_pid = pid(&b).await;
      a.batch_execute("BEGIN").await.expect("begin");
      a.execute(
        "SELECT 1 FROM flocks WHERE id = $1 FOR NO KEY UPDATE",
        &[&flock],
      )
      .await
      .expect("flock locked");
      let second = tokio::spawn(async move {
        update_rule(&mut b, &access(flock), theirs, None, None, Some(true))
          .await
          .expect("update")
      });
      wait_until_blocked(b_pid).await;
      a.execute("UPDATE rules SET enabled = true WHERE id = $1", &[&mine])
        .await
        .expect("first enable");
      a.batch_execute("COMMIT").await.expect("commit");
      assert!(matches!(
        second.await.expect("second caller"),
        UpdateOutcome::Refused(EnableRefusal::FlockFull)
      ));
      drop((a, setup));
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_deletes_only_a_rule_no_script_holds() {
    run(async {
      let db = Scratch::new("admin_delete").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let a = access(flock);
      let draft = new_rule(&client, flock).await;
      let SaveOutcome::Saved(_) = save_version(
        &mut client,
        &a,
        draft,
        Uuid::from_u128(7),
        &code("x", RuleOrigin::Hand),
        None,
      )
      .await
      .expect("save") else {
        panic!("save refused");
      };
      let live = new_rule(&client, flock).await;
      deploy(&client, live, 1).await;
      let uploading = new_rule(&client, flock).await;
      client
        .execute(
          "UPDATE rules SET pending_version = 1 WHERE id = $1",
          &[&uploading],
        )
        .await
        .expect("pending");

      let elsewhere = access(new_flock(&client, None).await);
      assert_eq!(
        delete_unscripted_rule(&client, &elsewhere, draft)
          .await
          .expect("delete"),
        DeleteOutcome::NotFound
      );
      for held in [live, uploading] {
        assert_eq!(
          delete_unscripted_rule(&client, &a, held)
            .await
            .expect("delete"),
          DeleteOutcome::InScript
        );
      }
      assert_eq!(
        delete_unscripted_rule(&client, &a, draft)
          .await
          .expect("delete"),
        DeleteOutcome::Deleted
      );
      let left: i64 = client
        .query_one(
          "SELECT (SELECT count(*) FROM rules) + (SELECT count(*) FROM rule_versions)",
          &[],
        )
        .await
        .expect("count")
        .get(0);
      assert_eq!(left, 2);
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_release_reads_the_test_record_just_written() {
    run(async {
      let db = Scratch::new("admin_release").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let rule = new_rule(&client, flock).await;
      let a = access(flock);
      let SaveOutcome::Saved(saved) = save_version(
        &mut client,
        &a,
        rule,
        Uuid::from_u128(7),
        &code("x", RuleOrigin::Hand),
        None,
      )
      .await
      .expect("save") else {
        panic!("save refused");
      };
      let before = read_release(&mut client, &a, rule, 1)
        .await
        .expect("release")
        .expect("found");
      assert_eq!(deploy_refusal(&before), Some(DeployRefusal::Untested));
      client
        .execute(
          "UPDATE rule_versions SET tested_sha256 = $3, tested_contract = $4, tested_at = now()
           WHERE rule_id = $1 AND version = $2",
          &[&rule, &1i32, &saved.sha256, &(RULE_CONTRACT as i32)],
        )
        .await
        .expect("tested");
      let after = read_release(&mut client, &a, rule, 1)
        .await
        .expect("release")
        .expect("found");
      assert_eq!(deploy_refusal(&after), None);
      assert!(
        read_release(&mut client, &a, rule, 2)
          .await
          .expect("release")
          .is_none()
      );
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_watch_lengthens_never_shortens_and_stays_inside_the_flock() {
    run(async {
      let db = Scratch::new("admin_watch").await;
      let client = db.client().await;
      let flock = new_flock(&client, None).await;
      let rule = new_rule(&client, flock).await;
      let a = access(flock);
      let watched = |r: &Rule| r.watch_until.expect("watched") - OffsetDateTime::now_utc();

      let first = watch_rule(&client, &a, rule)
        .await
        .expect("watch")
        .expect("found");
      let left = watched(&first);
      assert!(left > time::Duration::minutes(59) && left <= time::Duration::hours(1));

      client
        .execute(
          "UPDATE rules SET watch_until = now() + interval '24 hours' WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("deploy watch");
      let kept = watch_rule(&client, &a, rule)
        .await
        .expect("watch")
        .expect("found");
      assert!(watched(&kept) > time::Duration::hours(23));

      let elsewhere = access(new_flock(&client, None).await);
      assert!(
        watch_rule(&client, &elsewhere, rule)
          .await
          .expect("watch")
          .is_none()
      );
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_values_read_the_latest_per_pigeon_and_key() {
    run(async {
      let db = Scratch::new("admin_values").await;
      let client = db.client().await;
      let flock = new_flock(&client, None).await;
      let rule = new_rule(&client, flock).await;
      let other = new_rule(&client, flock).await;
      for pigeon in ["p1", "p2"] {
        client
          .execute(
            "INSERT INTO pigeons (id, flock_id, connector, created_at, updated_at)
             VALUES ($1, $2, '{}', now(), now())",
            &[&pigeon, &flock],
          )
          .await
          .expect("pigeon");
      }
      for (owner, pigeon, key, value, num, at) in [
        (rule, "p1", "dwell_s", "10", Some(10.0), 100.0),
        (rule, "p1", "dwell_s", "12", Some(12.0), 200.0),
        (rule, "p1", "state", "open", None, 150.0),
        (rule, "p2", "dwell_s", "7", Some(7.0), 50.0),
        (other, "p1", "dwell_s", "99", Some(99.0), 300.0),
      ] {
        client
          .execute(
            "INSERT INTO rule_values (rule_id, pigeon_id, key, value, value_num, at)
             VALUES ($1, $2, $3, $4, $5, to_timestamp($6))",
            &[&owner, &pigeon, &key, &value, &num, &at],
          )
          .await
          .expect("value");
      }
      let values = latest_values(&client, &access(flock), rule)
        .await
        .expect("values")
        .expect("found");
      let seen: Vec<(&str, &str, &str)> = values
        .iter()
        .map(|v| (v.pigeon_id.as_str(), v.key.as_str(), v.value.as_str()))
        .collect();
      assert_eq!(
        seen,
        [
          ("p1", "dwell_s", "12"),
          ("p1", "state", "open"),
          ("p2", "dwell_s", "7")
        ]
      );
      assert_eq!(values[0].value_num, Some(12.0));
      assert_eq!(values[1].value_num, None);

      let elsewhere = access(new_flock(&client, None).await);
      assert!(
        latest_values(&client, &elsewhere, rule)
          .await
          .expect("values")
          .is_none()
      );
      drop(client);
      db.drop().await;
    });
  }

  /// A new organization's id.
  async fn new_org(client: &Client) -> Uuid {
    client
      .query_one(
        "INSERT INTO organizations (name) VALUES ('o') RETURNING id",
        &[],
      )
      .await
      .expect("organization")
      .get(0)
  }

  /// Version `version` of `rule` with `source`, untested.
  async fn version_of(client: &Client, rule: Uuid, version: i32, source: &str) {
    client
      .execute(
        "INSERT INTO rule_versions (rule_id, version, source, sha256, contract, origin, created_by)
         VALUES ($1, $2, $3, 'x', 1, 'hand', gen_random_uuid())",
        &[&rule, &version, &source],
      )
      .await
      .expect("version");
  }

  /// A rule of `flock` running `version`, whose source is `source`.
  async fn running(client: &Client, flock: Uuid, version: i32, source: &str) -> Uuid {
    let rule = new_rule(client, flock).await;
    version_of(client, rule, version, source).await;
    client
      .execute(
        "UPDATE rules SET deployed_version = $2, enabled = true WHERE id = $1",
        &[&rule, &version],
      )
      .await
      .expect("running");
    rule
  }

  async fn rule_now(client: &Client, rule: Uuid) -> Option<RuleRow> {
    client
      .query_opt(
        concat!("SELECT ", rule_columns!(), " FROM rules r WHERE r.id = $1"),
        &[&rule],
      )
      .await
      .expect("rule readable")
      .as_ref()
      .map(rule_row)
  }

  async fn bundle_now(client: &Client, flock: Uuid) -> Option<(String, bool, bool)> {
    client
      .query_opt(
        "SELECT account_tag, uploaded_at IS NOT NULL, deploy_until > now()
         FROM rule_bundles WHERE flock_id = $1",
        &[&flock],
      )
      .await
      .expect("bundle readable")
      .map(|row| (row.get(0), row.get(1), row.get(2)))
  }

  fn keys(sources: &[BundleSource]) -> Vec<(Uuid, i32, &str)> {
    sources
      .iter()
      .map(|s| (s.rule_id, s.version, s.source.as_str()))
      .collect()
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_deploy_uploads_under_a_lease_and_promotes_on_success() {
    run(async {
      let db = Scratch::new("admin_deploy").await;
      let mut client = db.client().await;
      let account = new_org(&client).await;
      let flock = new_flock(&client, Some(account)).await;
      let sibling = running(&client, flock, 3, "sib").await;
      let paused = running(&client, flock, 1, "paused").await;
      client
        .execute(
          "UPDATE rules SET enabled = false, pending_version = 2 WHERE id = $1",
          &[&paused],
        )
        .await
        .expect("paused, an unconfirmed deploy of it pending");
      let blocked = running(&client, flock, 1, "blocked").await;
      client
        .execute(
          "UPDATE rules SET blocked_at = now() WHERE id = $1",
          &[&blocked],
        )
        .await
        .expect("blocked");
      let rule = new_rule(&client, flock).await;
      version_of(&client, rule, 1, "one").await;
      version_of(&client, rule, 2, "two").await;
      client
        .execute(
          "INSERT INTO rule_state (rule_id, scope, doc) VALUES ($1, '', '{\"n\":1}')",
          &[&rule],
        )
        .await
        .expect("state");
      let a = access(flock);

      let DeployStart::Started { lease, sources } = start_deploy(&mut client, &a, rule, 2, account)
        .await
        .expect("start")
      else {
        panic!("the lease was free");
      };
      assert_eq!(
        keys(&sources),
        [(sibling, 3, "sib"), (paused, 1, "paused"), (rule, 2, "two")],
        "every deployed rule at its deployed version, paused or not, and the candidate; the \
         blocked rule left out"
      );
      assert_eq!(
        rule_now(&client, rule).await.unwrap().pending_version,
        Some(2)
      );
      let tag = account_tag(&uuid_str(account));
      assert_eq!(
        bundle_now(&client, flock).await,
        Some((tag.clone(), false, true))
      );
      assert!(matches!(
        start_deploy(&mut client, &a, sibling, 3, account)
          .await
          .expect("second"),
        DeployStart::Busy
      ));
      let elsewhere = access(new_flock(&client, Some(account)).await);
      assert!(matches!(
        start_deploy(&mut client, &elsewhere, rule, 2, account)
          .await
          .expect("other flock"),
        DeployStart::Gone
      ));

      let DeployFinish::Promoted(done) =
        finish_deploy(&mut client, &a, rule, 2, lease, true, account)
          .await
          .expect("finish")
      else {
        panic!("the lease was held");
      };
      assert_eq!(
        (done.deployed_version, done.pending_version),
        (Some(2), None)
      );
      assert!(done.enabled && done.watch_until.is_some());
      let state: i64 = client
        .query_one(
          "SELECT count(*) FROM rule_state WHERE rule_id = $1",
          &[&rule],
        )
        .await
        .expect("count")
        .get(0);
      assert_eq!(state, 0, "reset on deploy");
      assert_eq!(
        rule_now(&client, paused).await.unwrap().pending_version,
        None,
        "the upload built the sibling at its deployed version"
      );
      assert_eq!(bundle_now(&client, flock).await, Some((tag, true, false)));
      assert!(matches!(
        start_deploy(&mut client, &a, rule, 1, account)
          .await
          .expect("next"),
        DeployStart::Started { .. }
      ));
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_refused_deploy_frees_the_lease_and_an_unsure_one_lets_it_lapse() {
    run(async {
      let db = Scratch::new("admin_deploy_fail").await;
      let mut client = db.client().await;
      let account = new_org(&client).await;
      let flock = new_flock(&client, Some(account)).await;
      let rule = new_rule(&client, flock).await;
      version_of(&client, rule, 1, "one").await;
      let a = access(flock);
      let DeployStart::Started { lease, .. } = start_deploy(&mut client, &a, rule, 1, account)
        .await
        .expect("start")
      else {
        panic!("free");
      };
      abandon_deploy(&mut client, &a, rule, 1, lease)
        .await
        .expect("abandon");
      let now = rule_now(&client, rule).await.unwrap();
      assert_eq!((now.pending_version, now.deployed_version), (None, None));
      assert!(!bundle_now(&client, flock).await.unwrap().2, "lease freed");

      // An unsure upload is not abandoned: the lease and the pending version stay.
      let DeployStart::Started { .. } = start_deploy(&mut client, &a, rule, 1, account)
        .await
        .expect("start")
      else {
        panic!("free");
      };
      assert!(matches!(
        start_deploy(&mut client, &a, rule, 1, account)
          .await
          .expect("retry"),
        DeployStart::Busy
      ));
      let swept = |rows: Vec<SweepBundle>| {
        let row = rows.into_iter().find(|r| r.flock_id == flock).expect("row");
        (row.leased, row.stale_pending)
      };
      assert_eq!(
        swept(sweep_bundles(&client).await.expect("read")),
        (true, false)
      );
      for (lapsed, stale) in [("5 minutes", false), ("11 minutes", true)] {
        let sql = format!(
          "UPDATE rule_bundles SET deploy_until = now() - interval '{lapsed}' WHERE flock_id = $1"
        );
        client.execute(&sql, &[&flock]).await.expect("lapsed");
        assert_eq!(
          swept(sweep_bundles(&client).await.expect("read")),
          (false, stale),
          "lapsed {lapsed}"
        );
      }
      assert_eq!(
        rule_now(&client, rule).await.unwrap().pending_version,
        Some(1)
      );
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_deploy_that_lost_its_lease_promotes_nothing() {
    run(async {
      let db = Scratch::new("admin_deploy_lost").await;
      let mut client = db.client().await;
      let account = new_org(&client).await;
      let flock = new_flock(&client, Some(account)).await;
      let rule = new_rule(&client, flock).await;
      version_of(&client, rule, 1, "one").await;
      let a = access(flock);
      let DeployStart::Started { lease, .. } = start_deploy(&mut client, &a, rule, 1, account)
        .await
        .expect("start")
      else {
        panic!("free");
      };
      client
        .execute(
          "UPDATE rule_bundles SET deploy_until = now() + interval '1 minute 1 second'
           WHERE flock_id = $1",
          &[&flock],
        )
        .await
        .expect("another deploy's lease");
      assert!(matches!(
        finish_deploy(&mut client, &a, rule, 1, lease, false, account)
          .await
          .expect("finish"),
        DeployFinish::LeaseLost
      ));
      let now = rule_now(&client, rule).await.unwrap();
      assert_eq!((now.deployed_version, now.pending_version), (None, None));
      assert!(!now.enabled);
      assert_eq!(
        bundle_now(&client, flock).await.unwrap().2,
        true,
        "not ours to free"
      );
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_deploy_into_a_full_flock_promotes_without_enabling() {
    run(async {
      let db = Scratch::new("admin_deploy_full").await;
      let mut client = db.client().await;
      let account = new_org(&client).await;
      let flock = new_flock(&client, Some(account)).await;
      for _ in 0..MAX_ENABLED_RULES_PER_FLOCK {
        running(&client, flock, 1, "s").await;
      }
      let rule = new_rule(&client, flock).await;
      version_of(&client, rule, 1, "one").await;
      let a = access(flock);
      let DeployStart::Started { lease, sources } = start_deploy(&mut client, &a, rule, 1, account)
        .await
        .expect("start")
      else {
        panic!("free");
      };
      assert_eq!(sources.len() as i64, MAX_ENABLED_RULES_PER_FLOCK + 1);
      let DeployFinish::Promoted(done) =
        finish_deploy(&mut client, &a, rule, 1, lease, false, account)
          .await
          .expect("finish")
      else {
        panic!("held");
      };
      assert_eq!(done.deployed_version, Some(1));
      assert!(!done.enabled, "the flock filled while it uploaded");
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_scripted_delete_disables_first_and_rebuilds_without_the_rule() {
    run(async {
      let db = Scratch::new("admin_rebuild").await;
      let mut client = db.client().await;
      let account = new_org(&client).await;
      let flock = new_flock(&client, Some(account)).await;
      let keep = running(&client, flock, 2, "keep").await;
      let idle = running(&client, flock, 1, "idle").await;
      client
        .execute("UPDATE rules SET enabled = false WHERE id = $1", &[&idle])
        .await
        .expect("disabled");
      let gone = running(&client, flock, 1, "gone").await;
      let RebuildStart::Started { lease, sources } =
        start_rebuild(&mut client, flock, Some(gone), account)
          .await
          .expect("start")
      else {
        panic!("free");
      };
      assert_eq!(keys(&sources), [(keep, 2, "keep"), (idle, 1, "idle")]);
      assert!(
        !rule_now(&client, gone).await.unwrap().enabled,
        "disabled before any upload"
      );
      assert!(matches!(
        start_rebuild(&mut client, flock, Some(keep), account)
          .await
          .expect("second"),
        RebuildStart::Busy
      ));
      assert!(
        finish_rebuild(&mut client, flock, lease, Some(gone), false, account)
          .await
          .expect("finish")
      );
      assert!(rule_now(&client, gone).await.is_none());
      assert_eq!(bundle_now(&client, flock).await.unwrap().1, true);

      let RebuildStart::Started { lease, sources } =
        start_rebuild(&mut client, flock, Some(keep), account)
          .await
          .expect("last enabled")
      else {
        panic!("free");
      };
      assert_eq!(
        keys(&sources),
        [(idle, 1, "idle")],
        "a disabled rule keeps the script, so enabling it again has a module to run"
      );
      assert!(
        finish_rebuild(&mut client, flock, lease, Some(keep), false, account)
          .await
          .expect("finish")
      );

      let RebuildStart::Started { lease, sources } =
        start_rebuild(&mut client, flock, Some(idle), account)
          .await
          .expect("last")
      else {
        panic!("free");
      };
      assert!(sources.is_empty());
      assert!(
        finish_rebuild(&mut client, flock, lease, Some(idle), true, account)
          .await
          .expect("finish")
      );
      assert!(rule_now(&client, keep).await.is_none());
      assert!(rule_now(&client, idle).await.is_none());
      assert_eq!(
        bundle_now(&client, flock).await,
        None,
        "no script, no bundle row"
      );
      assert!(matches!(
        start_rebuild(&mut client, flock, Some(keep), account)
          .await
          .expect("gone"),
        RebuildStart::NotFound
      ));
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_no_path_pipelines_a_request_behind_a_rollback() {
    run(async {
      let db = Scratch::new("admin_piped").await;
      let setup = db.client().await;
      let account = new_org(&setup).await;
      let flock = new_flock(&setup, Some(account)).await;
      let a = access(flock);
      let gone = running(&setup, flock, 1, "gone").await;
      let (mut client, piped) = pipeline_watched(&db).await;
      let mut seen = Vec::new();

      // The scripted delete: classify, then rebuild on the same client.
      assert_eq!(
        delete_unscripted_rule(&client, &a, gone)
          .await
          .expect("classify"),
        DeleteOutcome::InScript
      );
      let RebuildStart::Started { lease, sources } =
        start_rebuild(&mut client, flock, Some(gone), account)
          .await
          .expect("start")
      else {
        panic!("free");
      };
      assert!(sources.is_empty());
      assert!(!rule_now(&setup, gone).await.unwrap().enabled);
      assert!(
        finish_rebuild(&mut client, flock, lease, Some(gone), true, account)
          .await
          .expect("finish")
      );
      seen.push(("classify, rebuild", piped.load(Ordering::SeqCst)));

      // A rebuild and a deploy for a rule deleted under them, each freeing its lease after.
      assert!(matches!(
        start_rebuild(&mut client, flock, Some(gone), account)
          .await
          .expect("rebuild"),
        RebuildStart::NotFound
      ));
      seen.push(("rebuild of a gone rule", piped.load(Ordering::SeqCst)));
      assert!(matches!(
        start_deploy(&mut client, &a, gone, 1, account)
          .await
          .expect("deploy"),
        DeployStart::Gone
      ));
      seen.push(("deploy of a gone rule", piped.load(Ordering::SeqCst)));

      // A finish whose lease lapsed, then the sweep's next statement on the same client.
      assert!(
        !finish_rebuild(&mut client, flock, lease, None, false, account)
          .await
          .expect("finish")
      );
      assert_eq!(
        flock_owner(&client, flock).await.expect("owner"),
        Some(account)
      );
      seen.push(("lapsed finish", piped.load(Ordering::SeqCst)));

      // A route's refusal, then a statement on the same client.
      assert!(matches!(
        update_rule(&mut client, &a, gone, Some("n"), None, None)
          .await
          .expect("update"),
        UpdateOutcome::NotFound
      ));
      assert_eq!(
        flock_owner(&client, flock).await.expect("owner"),
        Some(account)
      );
      seen.push(("refused update", piped.load(Ordering::SeqCst)));

      assert!(
        seen.iter().all(|(_, n)| *n == 0),
        "requests sent behind a ROLLBACK, cumulative: {seen:?}"
      );
      drop(client);
      drop(setup);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_the_sweep_reads_each_bundle_against_its_owner_and_leases() {
    run(async {
      let db = Scratch::new("admin_sweep").await;
      let client = db.client().await;
      let owner = new_org(&client).await;
      let moved = Uuid::from_u128(0xacc8);
      let calm = new_flock(&client, Some(owner)).await;
      let stuck = new_flock(&client, Some(owner)).await;
      let busy = new_flock(&client, Some(owner)).await;
      for (flock, tag, until) in [
        (calm, account_tag(&uuid_str(moved)), "'epoch'"),
        (
          stuck,
          account_tag(&uuid_str(owner)),
          "now() - interval '11 minutes'",
        ),
        (
          busy,
          account_tag(&uuid_str(owner)),
          "now() + interval '1 minute'",
        ),
      ] {
        let sql = format!(
          "INSERT INTO rule_bundles (flock_id, contract, account_tag, uploaded_at, deploy_until)
           VALUES ($1, 1, $2, now(), {until})"
        );
        client.execute(&sql, &[&flock, &tag]).await.expect("bundle");
      }
      let rule = new_rule(&client, stuck).await;
      client
        .execute(
          "UPDATE rules SET pending_version = 4 WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("pending");
      let mut rows = sweep_bundles(&client).await.expect("read");
      rows.sort_by_key(|r| [calm, stuck, busy].iter().position(|f| *f == r.flock_id));
      let owner_tag = account_tag(&uuid_str(owner));
      let seen: Vec<(Uuid, &str, Uuid, bool, bool)> = rows
        .iter()
        .map(|r| {
          (
            r.flock_id,
            r.account_tag.as_str(),
            r.owner,
            r.leased,
            r.stale_pending,
          )
        })
        .collect();
      assert_eq!(
        seen,
        [
          (
            calm,
            account_tag(&uuid_str(moved)).as_str(),
            owner,
            false,
            false
          ),
          (stuck, owner_tag.as_str(), owner, false, true),
          (busy, owner_tag.as_str(), owner, true, false),
        ]
      );
      assert_eq!(flock_owner(&client, calm).await.expect("read"), Some(owner));
      retag_bundle(&client, calm, owner).await.expect("retag");
      assert_eq!(bundle_now(&client, calm).await.unwrap().0, owner_tag);
      assert!(bundle_uploaded(&client, calm).await.expect("read"));
      assert!(
        !bundle_uploaded(&client, new_flock(&client, None).await)
          .await
          .expect("read")
      );
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_admin_a_dry_run_reads_the_would_be_bundle_and_records_only_the_bytes_it_ran() {
    use crate::helpers::rule_store::pg::new_pigeon;
    use crate::helpers::rule_store::replay_names;
    use crate::helpers::rules::FIELD_PIGEONS;
    run(async {
      let db = Scratch::new("admin_dry_run").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let a = access(flock);
      let author = Uuid::from_u128(7);
      let save = async |client: &mut Client, rule, source: &str| match save_version(
        client,
        &a,
        rule,
        author,
        &code(source, RuleOrigin::Hand),
        None,
      )
      .await
      .expect("save")
      {
        SaveOutcome::Saved(v) => v,
        _ => panic!("save refused"),
      };
      // Created in this order: a live sibling, the rule under test, a disabled one, a blocked one.
      let sibling = new_rule(&client, flock).await;
      let rule = new_rule(&client, flock).await;
      let disabled = new_rule(&client, flock).await;
      let blocked = new_rule(&client, flock).await;
      save(&mut client, sibling, "sibling").await;
      save(&mut client, rule, "old").await;
      let candidate = save(&mut client, rule, "new").await;
      save(&mut client, disabled, "disabled").await;
      save(&mut client, blocked, "blocked").await;
      for id in [sibling, rule, disabled, blocked] {
        deploy(&client, id, 1).await;
      }
      client
        .execute(
          "UPDATE rules SET enabled = true WHERE id = ANY($1)",
          &[&vec![sibling, blocked]],
        )
        .await
        .expect("enabled");
      client
        .execute(
          "UPDATE rules SET blocked_at = now() WHERE id = $1",
          &[&blocked],
        )
        .await
        .expect("blocked");
      for (id, scope, doc) in [
        (rule, "", r#"{"seen":3}"#),
        (rule, "p1", r#"{"n":1}"#),
        (rule, "p2", r#"{"n":2}"#),
        (sibling, "p1", r#"{"s":1}"#),
      ] {
        client
          .execute(
            "INSERT INTO rule_state (rule_id, scope, doc) VALUES ($1, $2, $3::text::jsonb)",
            &[&id, &scope, &doc],
          )
          .await
          .expect("state");
      }

      let release = read_release(&mut client, &a, rule, 2)
        .await
        .expect("read")
        .expect("found");
      let sources = dry_run_sources(&client, &a, &release, &["p1".to_string()])
        .await
        .expect("read")
        .expect("found");
      assert_eq!(sources.candidate, "new");
      assert_eq!(sources.deployed, Some((1, "old".to_string())));
      let bundle: Vec<(Uuid, i32)> = sources
        .bundle
        .iter()
        .map(|s| (s.rule_id, s.version))
        .collect();
      assert_eq!(
        bundle,
        [(sibling, 1), (rule, 2)],
        "enabled siblings and the candidate in its place; never disabled or blocked rules"
      );
      let seed = &sources.seeds[&rule];
      assert_eq!(seed.flock["seen"], 3);
      assert_eq!(
        seed.pigeons.keys().collect::<Vec<_>>(),
        ["p1"],
        "only the replay's pigeons"
      );
      assert_eq!(sources.seeds[&sibling].pigeons["p1"]["s"], 1);

      assert!(deploy_refusal(&release) == Some(DeployRefusal::Untested));
      let summary = r#"{"window":{"from":null,"to":null,"readings":0,"batches":0,"pigeons":0,
        "cut":false},"candidate":{"version":2,"runs":0,"ok":0,"threw":0,"timeout":0,
        "bad_output":0,"values":0,"mails":[],"mails_total":0,"errors":[],"logs":[],
        "stopped_at":null,"diverged_at":null,"pigeon_state_bytes":0,"flock_state_bytes":0},
        "deployed":null,"seeded":null,"mails":{"new":[],"gone":[],"unchanged":0},"cases":[],
        "cpu":{"candidate":"not_run","bundle":"not_run","batches":0,"load_error":null}}"#;
      let stale = record_test(&client, &a, rule, 2, "not-these-bytes", summary)
        .await
        .expect("record");
      assert!(
        stale.is_none(),
        "a record counts only for the bytes that ran"
      );
      let elsewhere = access(new_flock(&client, None).await);
      let foreign = record_test(&client, &elsewhere, rule, 2, &candidate.sha256, summary)
        .await
        .expect("record");
      assert!(foreign.is_none());
      let tested = record_test(&client, &a, rule, 2, &candidate.sha256, summary)
        .await
        .expect("record")
        .expect("recorded");
      assert_eq!(tested.tested_contract, Some(RULE_CONTRACT as i32));
      assert!(tested.tested_at.is_some() && tested.source.as_deref() == Some("new"));
      assert_eq!(tested.test_summary.expect("typed").candidate.version, 2);
      let release = read_release(&mut client, &a, rule, 2)
        .await
        .expect("read")
        .expect("found");
      assert_eq!(
        deploy_refusal(&release),
        None,
        "the route's record is what deploy reads"
      );

      new_pigeon(&client, flock, "p1").await;
      new_pigeon(&client, flock, FIELD_PIGEONS[0]).await;
      let (name, names) = replay_names(&client, &a, None)
        .await
        .expect("read")
        .expect("found");
      assert_eq!(name, "f");
      assert!(names.contains_key("p1") && !names.contains_key(FIELD_PIGEONS[0]));
      drop(client);
      db.drop().await;
    });
  }
}
