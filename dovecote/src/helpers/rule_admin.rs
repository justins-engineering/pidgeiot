//! The statements behind the dashboard's rule routes, and the refusals they decide on.
//!
//! Every statement names the rule together with the flock a [`FlockAccess`] proved, so a rule id
//! from another flock reads as absent. A read that gates a mutation selects `now()`, which
//! Hyperdrive never caches, and runs inside the mutation's own transaction under the rule's row
//! lock, keeping the lock order of [`super::rule_store`]: an enable locks the flock's row first,
//! to count its enabled rules without a racing enable, then the rule's.

use capsules::{
  MAX_ENABLED_RULES_PER_FLOCK, MAX_RULE_RUNS_PER_PAGE, MAX_RULE_VALUES_RETURNED, MAX_RULE_VERSIONS,
  RULE_CONTRACT, Rule, RuleDetail, RuleOrigin, RuleRow, RuleRun, RuleRunRow, RuleValue,
  RuleValueRow, RuleVersion, RuleVersionRow,
};
use tokio_postgres::{Client, Row, Transaction, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

use crate::helpers::firmware::FlockAccess;
use crate::helpers::nidd::org_listed;
use crate::helpers::rule_store::ensure_rule_tables;
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

fn flock_uuid(access: &FlockAccess) -> Result<Uuid> {
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
  let tx = client
    .transaction()
    .await
    .map_err(db_error("Rule update begin error"))?;
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
  tx.commit()
    .await
    .map_err(db_error("Rule update commit error"))?;
  Ok(UpdateOutcome::Updated(Rule::from(rule_row(&row))))
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
pub async fn delete_unscripted_rule(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<DeleteOutcome> {
  ensure_rule_tables(client).await?;
  let flock = flock_uuid(access)?;
  let tx = client
    .transaction()
    .await
    .map_err(db_error("Rule delete begin error"))?;
  let Some(current) = lock_rule_row(&tx, flock, rule_id).await? else {
    return Ok(DeleteOutcome::NotFound);
  };
  if current.deployed_version.is_some() || current.pending_version.is_some() {
    return Ok(DeleteOutcome::InScript);
  }
  tx.query_typed("DELETE FROM rules WHERE id = $1", &[(&rule_id, Type::UUID)])
    .await
    .map_err(db_error("Rule delete error"))?;
  tx.commit()
    .await
    .map_err(db_error("Rule delete commit error"))?;
  Ok(DeleteOutcome::Deleted)
}

/// What a save did.
#[derive(Debug)]
pub enum SaveOutcome {
  /// The new version, with its source.
  Saved(RuleVersion),
  /// The flock has no such rule.
  NotFound,
  /// `based_on` names no version of this rule.
  NoBase,
}

/// Stores `source` as the rule's next version, then drops the oldest versions past
/// [`MAX_RULE_VERSIONS`] that are neither deployed nor uploading. `source` is already checked.
pub async fn save_version(
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  created_by: Uuid,
  source: &str,
  origin: RuleOrigin,
  based_on: Option<i32>,
) -> Result<SaveOutcome> {
  ensure_rule_tables(client).await?;
  let flock = flock_uuid(access)?;
  let sha256 = sha256_hex(source.as_bytes());
  let contract = RULE_CONTRACT as i32;
  let origin = origin.as_str();
  let tx = client
    .transaction()
    .await
    .map_err(db_error("Rule save begin error"))?;
  let Some(current) = lock_rule_row(&tx, flock, rule_id).await? else {
    return Ok(SaveOutcome::NotFound);
  };
  let Some(row) = tx
    .query_typed_opt(
      concat!(
        "INSERT INTO rule_versions AS v
           (rule_id, version, source, sha256, contract, origin, based_on, created_by)
         SELECT $1, COALESCE(MAX(version), 0) + 1, $2, $3, $4, $5, $6, $7
         FROM rule_versions WHERE rule_id = $1
         HAVING $6::integer IS NULL OR bool_or(version = $6)
         RETURNING ",
        version_columns!(),
        ", v.test_summary::text AS test_summary, v.source"
      ),
      &[
        (&rule_id, Type::UUID),
        (&source, Type::TEXT),
        (&sha256, Type::TEXT),
        (&contract, Type::INT4),
        (&origin, Type::TEXT),
        (&based_on, Type::INT4),
        (&created_by, Type::UUID),
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
  tx.commit()
    .await
    .map_err(db_error("Rule save commit error"))?;
  Ok(SaveOutcome::Saved(RuleVersion::from(version_row(&row))))
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
        ", v.test_summary::text AS test_summary, v.source
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
  let tx = client
    .transaction()
    .await
    .map_err(db_error("Rule release begin error"))?;
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
  tx.commit()
    .await
    .map_err(db_error("Rule release commit error"))?;
  Ok(Some(Release {
    rule,
    version: version_row(&version),
    enabled_siblings,
  }))
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
    Scratch, new_flock, new_rule, pid, run, wait_until_blocked,
  };
  use time::OffsetDateTime;

  fn access(flock: Uuid) -> FlockAccess {
    FlockAccess::assert_checked(&flock.to_string(), None)
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
        "export default () => ({});",
        RuleOrigin::Hand,
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
          "x",
          RuleOrigin::Hand,
          Some(9)
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
          "x",
          RuleOrigin::Hand,
          None
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
          "x",
          RuleOrigin::AssistantEdited,
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
        "x",
        RuleOrigin::Hand,
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
        delete_unscripted_rule(&mut client, &elsewhere, draft)
          .await
          .expect("delete"),
        DeleteOutcome::NotFound
      );
      for held in [live, uploading] {
        assert_eq!(
          delete_unscripted_rule(&mut client, &a, held)
            .await
            .expect("delete"),
          DeleteOutcome::InScript
        );
      }
      assert_eq!(
        delete_unscripted_rule(&mut client, &a, draft)
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
        "x",
        RuleOrigin::Hand,
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
}
