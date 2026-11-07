//! The Postgres side of user-authored rules: the tables, and the statements whose concurrency the
//! apply depends on. The pure plan in [`super::rules`] decides on a snapshot; each statement here
//! re-checks on the row it finds, so racing batches and deploys agree with the plan or do nothing.
//!
//! Lock order, which is what keeps that true: a transaction that writes a rule's state, flags or
//! mail takes the rule's `rules` row first ([`lock_rule`]), so two batches of one rule queue there
//! instead of locking flag rows crosswise; the only rows two rules share are `rule_usage`'s,
//! always taken account first, then platform. A deploy that resets state locks the `rules` row
//! before it deletes from `rule_state`. A transaction that counts a flock's enabled rules (an
//! enable, a deploy's promote) locks the flock's `flocks` row `FOR NO KEY UPDATE` before any
//! rule's row, and a deploy's promote or a rebuild's finish locks the flock's `rule_bundles` row
//! after the flock's and before any rule's.
#![cfg_attr(
  not(all(test, not(target_arch = "wasm32"))),
  expect(
    dead_code,
    reason = "its callers are the queue's second pass and the rule routes"
  )
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::helpers::missing_schema_objects;
use crate::helpers::rules::{
  ACCOUNT_RULE_MAIL_PER_DAY, AlertFlag, AlertFlags, FLOCK_FLAG_HOLD_SECS, FailStreak, LiveRule,
  Lookup, NOTIFY_REPEAT_SECS, PLATFORM_RULE_MAIL_PER_DAY, PlannedValue, RULE_MAIL_PER_DAY,
  RULE_MAIL_PER_HOUR, RuleMailCounters, RulePlan, RuleWork, StreakWrite, next_streak,
};
use capsules::{RuleAlert, RuleAlertKind, RuleAlertScope, RuleNamedRef, RuleState};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Transaction, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

use crate::helpers::firmware::FlockAccess;
use crate::helpers::rules::excluded_pigeons;
use crate::helpers::{TelemetryHistoryPage, query_telemetry_history_for_flock};

/// Every table and index [`RULE_TABLES_DDL`] creates, as `missing_schema_objects` names them.
const RULE_RELATIONS: [&str; 12] = [
  "rules",
  "idx_rules_flock_id",
  "rule_versions",
  "rule_state",
  "rule_alert_state",
  "rule_values",
  "idx_rule_values_pigeon_id",
  "rule_runs",
  "idx_rule_runs_rule_ran",
  "idx_rule_runs_pigeon_id",
  "rule_bundles",
  "rule_usage",
];

/// The rule tables, verbatim in `infra/migrations/2026-11-03-rules.sql` and `infra/init-db.sql`
/// (a test holds all three together). Idempotent, and creates no trigger: writes that change a
/// rule's definition set `updated_at` themselves, and the mail counters are not such a write.
pub const RULE_TABLES_DDL: &str = r#"CREATE TABLE IF NOT EXISTS rules (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  flock_id UUID NOT NULL REFERENCES flocks(id) ON DELETE CASCADE,
  created_by UUID NOT NULL,
  name TEXT NOT NULL,
  recipients JSONB NOT NULL DEFAULT '[]',
  enabled BOOLEAN NOT NULL DEFAULT false,
  deployed_version INTEGER,
  pending_version INTEGER,
  paused_reason TEXT,
  blocked_at TIMESTAMPTZ,
  blocked_reason TEXT,
  fail_count INTEGER NOT NULL DEFAULT 0,
  fail_since TIMESTAMPTZ,
  watch_until TIMESTAMPTZ,
  mail_hour TIMESTAMPTZ,
  mail_in_hour INTEGER NOT NULL DEFAULT 0,
  mail_day DATE,
  mail_in_day INTEGER NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- The dispatch gate joins a pigeon's flock to its rules on every batch.
CREATE INDEX IF NOT EXISTS idx_rules_flock_id ON rules(flock_id);
CREATE TABLE IF NOT EXISTS rule_versions (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  language TEXT NOT NULL DEFAULT 'js' CHECK (language = 'js'),
  source TEXT NOT NULL CHECK (octet_length(source) <= 65536),
  sha256 TEXT NOT NULL,
  contract INTEGER NOT NULL,
  origin TEXT NOT NULL CHECK (origin IN ('hand', 'assistant', 'assistant_edited')),
  based_on INTEGER,
  repair_depth SMALLINT NOT NULL DEFAULT 0,
  prompt TEXT,
  model TEXT,
  input_tokens INTEGER,
  output_tokens INTEGER,
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  tested_sha256 TEXT,
  tested_contract INTEGER,
  tested_at TIMESTAMPTZ,
  test_summary JSONB,
  created_by UUID NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (rule_id, version)
);
-- scope is '' for the flock document, else a pigeon id.
CREATE TABLE IF NOT EXISTS rule_state (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  scope TEXT NOT NULL,
  doc JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (rule_id, scope)
);
-- decided_at is the last reading of the batch that changed the flag, so an older batch never
-- undoes a newer one. last_sent_at is when a mail was claimed or suppressed, so a capped notify
-- also waits its week.
CREATE TABLE IF NOT EXISTS rule_alert_state (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  scope TEXT NOT NULL,
  key TEXT NOT NULL,
  firing BOOLEAN NOT NULL,
  decided_at TIMESTAMPTZ NOT NULL,
  changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_sent_at TIMESTAMPTZ,
  PRIMARY KEY (rule_id, scope, key)
);
CREATE TABLE IF NOT EXISTS rule_values (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  value_num DOUBLE PRECISION,
  at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (rule_id, pigeon_id, key, at)
);
-- Serves the cascade from a pigeon delete, which would otherwise scan the table.
CREATE INDEX IF NOT EXISTS idx_rule_values_pigeon_id ON rule_values(pigeon_id);
-- Keyed on the batch so a redelivered batch keeps the first delivery's row.
CREATE TABLE IF NOT EXISTS rule_runs (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  batch_id TEXT NOT NULL,
  status TEXT NOT NULL,
  readings INTEGER NOT NULL DEFAULT 0,
  alerts_sent INTEGER NOT NULL DEFAULT 0,
  alerts_suppressed INTEGER NOT NULL DEFAULT 0,
  values_written INTEGER NOT NULL DEFAULT 0,
  state_changed BOOLEAN NOT NULL DEFAULT false,
  wall_ms INTEGER,
  error TEXT,
  logs JSONB,
  ran_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (rule_id, pigeon_id, batch_id)
);
CREATE INDEX IF NOT EXISTS idx_rule_runs_rule_ran ON rule_runs(rule_id, ran_at);
CREATE INDEX IF NOT EXISTS idx_rule_runs_pigeon_id ON rule_runs(pigeon_id);
-- What is true of a flock's script rather than of one rule, and its deploy lease.
CREATE TABLE IF NOT EXISTS rule_bundles (
  flock_id UUID PRIMARY KEY REFERENCES flocks(id) ON DELETE CASCADE,
  contract INTEGER NOT NULL,
  account_tag TEXT NOT NULL,
  uploaded_at TIMESTAMPTZ,
  deploy_until TIMESTAMPTZ NOT NULL DEFAULT 'epoch'
);
-- scope is an account uuid or 'platform'. No foreign key: deleting a rule or a flock refunds
-- no assistant allowance and no mail.
CREATE TABLE IF NOT EXISTS rule_usage (
  scope TEXT NOT NULL,
  day DATE NOT NULL,
  assistant_turns INTEGER NOT NULL DEFAULT 0,
  input_tokens BIGINT NOT NULL DEFAULT 0,
  output_tokens BIGINT NOT NULL DEFAULT 0,
  cache_read_tokens BIGINT NOT NULL DEFAULT 0,
  cache_write_tokens BIGINT NOT NULL DEFAULT 0,
  cost_micro_usd BIGINT NOT NULL DEFAULT 0,
  mail_sent INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (scope, day)
);
"#;

/// The rule builder's column and origin, verbatim in `infra/migrations/2026-11-07-rule-builder.sql`
/// and `infra/init-db.sql`. Run after [`RULE_TABLES_DDL`], whose inline origin CHECK Postgres names
/// `rule_versions_origin_check`; idempotent, and one implicit transaction as one batch.
pub const RULE_BUILDER_DDL: &str =
  "ALTER TABLE rule_versions ADD COLUMN IF NOT EXISTS builder_model JSONB;
ALTER TABLE rule_versions DROP CONSTRAINT IF EXISTS rule_versions_origin_check;
ALTER TABLE rule_versions ADD CONSTRAINT rule_versions_origin_check
  CHECK (origin IN ('hand', 'assistant', 'assistant_edited', 'builder'));
";

/// The columns [`RULE_BUILDER_DDL`] adds to tables [`RULE_TABLES_DDL`] creates.
const RULE_COLUMNS: [(&str, &str); 1] = [("rule_versions", "builder_model")];

/// Set once this isolate has found every rule table and index.
static TABLES_READY: AtomicBool = AtomicBool::new(false);

/// Ensures, once per isolate, the rule tables, their indexes and the builder's column, logging
/// anything missing first: the migrations should already have created them.
pub async fn ensure_rule_tables(client: &Client) -> Result<()> {
  if TABLES_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  let missing = missing_schema_objects(client, &RULE_RELATIONS, &RULE_COLUMNS).await?;
  if missing.is_empty() {
    TABLES_READY.store(true, Ordering::Relaxed);
    return Ok(());
  }
  console_error!(
    "ensure_rule_tables: creating missing {}",
    missing.join(", ")
  );
  for ddl in [RULE_TABLES_DDL, RULE_BUILDER_DDL] {
    client.batch_execute(ddl).await.map_err(|e| {
      let code = e.code().map_or("none", |code| code.code());
      console_error!("ensure_rule_tables failed, SQLSTATE {code}");
      Error::RustError("Internal Server Error".into())
    })?;
  }
  TABLES_READY.store(true, Ordering::Relaxed);
  Ok(())
}

/// The `scope` column of `rule_state` and `rule_alert_state`: `''` for the flock, else the pigeon.
pub fn scope_key(scope: RuleAlertScope, pigeon_id: &str) -> &str {
  match scope {
    RuleAlertScope::Flock => "",
    RuleAlertScope::Pigeon => pigeon_id,
  }
}

/// `capsules::merge_state` on the row: each top-level key set, `null` deleting it. The update
/// reads the row it locks, so concurrent patches of different keys both land, and a patch that
/// changes nothing writes nothing. The per-document caps were checked on the snapshot; a
/// concurrent writer can overshoot them by its own patch, as design 1.4 accepts.
const MERGE_STATE_SQL: &str = "WITH p AS (
    SELECT $3::jsonb AS patch,
      ARRAY(SELECT k FROM jsonb_each($3::jsonb) AS e(k, v) WHERE v = 'null'::jsonb) AS gone
  )
  INSERT INTO rule_state AS s (rule_id, scope, doc)
  SELECT $1, $2, p.patch - p.gone FROM p WHERE p.patch - p.gone <> '{}'::jsonb
  ON CONFLICT (rule_id, scope) DO UPDATE
  SET doc = (s.doc || (SELECT patch FROM p)) - (SELECT gone FROM p), updated_at = now()
  WHERE s.doc <> (s.doc || (SELECT patch FROM p)) - (SELECT gone FROM p)
  RETURNING 1";

/// A `fire`: inserts the flag, or turns it on when the batch is newer than the one that last
/// changed it and, for the flock's flag, the flag has held [`FLOCK_FLAG_HOLD_SECS`]. Re-applies
/// `rules::transition_applies` to the row it locks, so of two racing fires one moves the flag.
const FIRE_SQL: &str = "INSERT INTO rule_alert_state AS s
    (rule_id, scope, key, firing, decided_at, changed_at, last_sent_at)
  VALUES ($1, $2, $3, true, to_timestamp($4), to_timestamp($5), to_timestamp($5))
  ON CONFLICT (rule_id, scope, key) DO UPDATE
  SET firing = true, decided_at = EXCLUDED.decided_at, changed_at = EXCLUDED.changed_at,
    last_sent_at = EXCLUDED.last_sent_at
  WHERE NOT s.firing AND EXCLUDED.decided_at > s.decided_at
    AND (s.scope <> '' OR s.changed_at <= EXCLUDED.changed_at - make_interval(secs => $6))
  RETURNING 1";

/// A `clear`: the same conditions as [`FIRE_SQL`]; with no flag there is nothing to clear.
const CLEAR_SQL: &str = "UPDATE rule_alert_state
  SET firing = false, decided_at = to_timestamp($4), changed_at = to_timestamp($5),
    last_sent_at = to_timestamp($5)
  WHERE rule_id = $1 AND scope = $2 AND key = $3 AND firing
    AND decided_at < to_timestamp($4)
    AND (scope <> '' OR changed_at <= to_timestamp($5) - make_interval(secs => $6))
  RETURNING 1";

/// A `notify`: once per key within [`NOTIFY_REPEAT_SECS`], leaving any fire/clear flag as it is.
const NOTIFY_SQL: &str = "INSERT INTO rule_alert_state AS s
    (rule_id, scope, key, firing, decided_at, changed_at, last_sent_at)
  VALUES ($1, $2, $3, false, to_timestamp($4), to_timestamp($5), to_timestamp($5))
  ON CONFLICT (rule_id, scope, key) DO UPDATE
  SET last_sent_at = EXCLUDED.last_sent_at
  WHERE s.last_sent_at IS NULL
    OR s.last_sent_at <= EXCLUDED.last_sent_at - make_interval(secs => $6)
  RETURNING 1";

/// One mail against the rule's hour and day, then the account's day, then the platform's day,
/// each counter moved only under its cap and only when the one before moved. All three are UTC.
///
/// A refusal can leave an earlier counter one higher than the mail sent, but only below a cap
/// that is already full for the rest of that day, so no mail is lost to it; the order is what
/// keeps an account at its cap from spending the platform's day.
const CLAIM_SQL: &str = "WITH t AS (
    SELECT date_trunc('hour', to_timestamp($3), 'UTC') AS hour,
      (to_timestamp($3) AT TIME ZONE 'UTC')::date AS day
  ), r AS (
    UPDATE rules SET
      mail_in_hour = CASE WHEN mail_hour = t.hour THEN mail_in_hour + 1 ELSE 1 END,
      mail_hour = t.hour,
      mail_in_day = CASE WHEN mail_day = t.day THEN mail_in_day + 1 ELSE 1 END,
      mail_day = t.day
    FROM t
    WHERE id = $1
      AND (mail_hour IS DISTINCT FROM t.hour OR mail_in_hour < $4)
      AND (mail_day IS DISTINCT FROM t.day OR mail_in_day < $5)
    RETURNING 1
  ), a AS (
    INSERT INTO rule_usage AS u (scope, day, mail_sent)
    SELECT $2::text, t.day, 1 FROM t WHERE EXISTS (SELECT 1 FROM r)
    ON CONFLICT (scope, day) DO UPDATE SET mail_sent = u.mail_sent + 1
    WHERE u.mail_sent < $6
    RETURNING 1
  ), p AS (
    INSERT INTO rule_usage AS u (scope, day, mail_sent)
    SELECT 'platform', t.day, 1 FROM t WHERE EXISTS (SELECT 1 FROM a)
    ON CONFLICT (scope, day) DO UPDATE SET mail_sent = u.mail_sent + 1
    WHERE u.mail_sent < $7
    RETURNING 1
  )
  SELECT EXISTS (SELECT 1 FROM p)";

/// Locks `rule_id`'s `rules` row for the rest of `tx`: the first lock of every transaction
/// that applies a rule's result (see the module's lock order). False when the rule is gone, and
/// then the transaction writes nothing.
pub async fn lock_rule(
  tx: &Transaction<'_>,
  rule_id: Uuid,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let rows = tx
    .query_typed(
      "SELECT 1 FROM rules WHERE id = $1 FOR UPDATE",
      &[(&rule_id, Type::UUID)],
    )
    .await?;
  Ok(!rows.is_empty())
}

/// What bounds every rule transaction ([`begin`]): a lock wait or a statement past these answers
/// an error instead of holding the request, and its pooled connection, until something gives.
const TX_LIMITS_SQL: &str = "SET LOCAL lock_timeout = '2s'; SET LOCAL statement_timeout = '10s'";

/// Opens a transaction under [`TX_LIMITS_SQL`]. End it with [`end`], never by dropping it.
pub(crate) async fn begin(
  client: &mut Client,
) -> std::result::Result<Transaction<'_>, tokio_postgres::Error> {
  let tx = client.transaction().await?;
  if let Err(e) = tx.batch_execute(TX_LIMITS_SQL).await {
    let _ = tx.rollback().await;
    return Err(e);
  }
  Ok(tx)
}

/// Ends `tx` on its body's answer, committing an `Ok` and rolling back an `Err`, and waits for
/// the server's reply either way. A dropped `Transaction` sends its `ROLLBACK` without waiting,
/// so the client's next request goes out behind it, and through Hyperdrive that pair lost a
/// reply: the next statement waited forever. Every `Ok` commits, early returns included, so a
/// body returns early either before it writes or with writes it means to keep.
pub(crate) async fn end<T, E>(
  tx: Transaction<'_>,
  body: std::result::Result<T, E>,
  commit_error: impl FnOnce(tokio_postgres::Error) -> E,
) -> std::result::Result<T, E> {
  match body {
    Ok(value) => tx.commit().await.map(|()| value).map_err(commit_error),
    Err(e) => {
      // The body's error is the one to report; a failed rollback means a broken connection.
      let _ = tx.rollback().await;
      Err(e)
    }
  }
}

/// True for a deadlock or serialization failure: one rule's apply losing a race, which the caller
/// counts as that rule's `dispatch_failed` rather than as Postgres being down.
pub fn is_contention(error: &tokio_postgres::Error) -> bool {
  error.code().is_some_and(|code| {
    *code == SqlState::T_R_DEADLOCK_DETECTED || *code == SqlState::T_R_SERIALIZATION_FAILURE
  })
}

/// Merges `patch` into the rule's document for `scope` ([`scope_key`]). True when it wrote; a
/// patch of only `null`s on a missing document writes nothing.
pub async fn merge_state_row(
  tx: &Transaction<'_>,
  rule_id: Uuid,
  scope: &str,
  patch: &Map<String, Value>,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let patch = Value::Object(patch.clone()).to_string();
  let rows = tx
    .query_typed(
      MERGE_STATE_SQL,
      &[
        (&rule_id, Type::UUID),
        (&scope, Type::TEXT),
        (&patch, Type::TEXT),
      ],
    )
    .await?;
  Ok(!rows.is_empty())
}

/// Applies one alert of a result to its flag: `last_reading` is the batch's last reading and
/// `now` dovecote's clock, both unix seconds. True when the flag moved or a `notify` is due, which
/// is when the caller claims its mail with [`claim_mail`]; false for a restatement, an older batch
/// or a held flock flag.
pub async fn apply_alert(
  tx: &Transaction<'_>,
  rule_id: Uuid,
  scope: &str,
  key: &str,
  kind: RuleAlertKind,
  last_reading: i64,
  now: i64,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let (sql, window) = match kind {
    RuleAlertKind::Fire => (FIRE_SQL, FLOCK_FLAG_HOLD_SECS),
    RuleAlertKind::Clear => (CLEAR_SQL, FLOCK_FLAG_HOLD_SECS),
    RuleAlertKind::Notify => (NOTIFY_SQL, NOTIFY_REPEAT_SECS),
  };
  let window = window as f64;
  let rows = tx
    .query_typed(
      sql,
      &[
        (&rule_id, Type::UUID),
        (&scope, Type::TEXT),
        (&key, Type::TEXT),
        (&last_reading, Type::INT8),
        (&now, Type::INT8),
        (&window, Type::FLOAT8),
      ],
    )
    .await?;
  Ok(!rows.is_empty())
}

/// Claims one mail for `rule_id` at `now` under the rule, account and platform caps. `account`
/// is the flock owner's organization id, else its user id. False is a suppressed mail.
pub async fn claim_mail(
  tx: &Transaction<'_>,
  rule_id: Uuid,
  account: Uuid,
  now: i64,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let row = tx
    .query_typed_one(
      CLAIM_SQL,
      &[
        (&rule_id, Type::UUID),
        (&account, Type::UUID),
        (&now, Type::INT8),
        (&RULE_MAIL_PER_HOUR, Type::INT4),
        (&RULE_MAIL_PER_DAY, Type::INT4),
        (&ACCOUNT_RULE_MAIL_PER_DAY, Type::INT4),
        (&PLATFORM_RULE_MAIL_PER_DAY, Type::INT4),
      ],
    )
    .await?;
  Ok(row.get(0))
}

/// The second pass's gate: whether a pigeon's flock has any live rule. It carries no `now()`, so
/// Hyperdrive caches it and a flock without rules costs one cache hit per batch.
const GATE_SQL: &str = "SELECT 1 FROM rules r JOIN pigeons p ON p.flock_id = r.flock_id
  WHERE p.id = $1 AND r.enabled AND r.blocked_at IS NULL AND p.suspended_at IS NULL LIMIT 1";

/// Whether `pigeon_id`'s flock has an enabled, unblocked rule and the pigeon is not suspended.
pub async fn rules_gate(
  client: &Client,
  pigeon_id: &str,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let rows = client
    .query_typed(GATE_SQL, &[(&pigeon_id, Type::TEXT)])
    .await?;
  Ok(!rows.is_empty())
}

/// The second pass's fresh read: one row per live rule of the pigeon's flock, or one row with no
/// rule. Its `now()` keeps Hyperdrive from caching it, so a disable, a block or a deploy acts on
/// the next batch. Every column is named: nothing here reads a connector or an endpoint.
const FRESH_READ_SQL: &str = "SELECT floor(extract(epoch FROM now()))::int8,
    f.id, f.name, COALESCE(p.name, p.id), COALESCE(f.org_id, f.user_id), f.org_id,
    floor(extract(epoch FROM b.uploaded_at))::int8, COALESCE(b.deploy_until > now(), false),
    COALESCE((SELECT u.mail_sent FROM rule_usage u
      WHERE u.scope = COALESCE(f.org_id, f.user_id)::text
        AND u.day = (now() AT TIME ZONE 'UTC')::date), 0),
    COALESCE((SELECT u.mail_sent FROM rule_usage u WHERE u.scope = 'platform'
      AND u.day = (now() AT TIME ZONE 'UTC')::date), 0),
    r.id, r.deployed_version, r.fail_count > 0, COALESCE(r.watch_until > now(), false),
    r.pending_version IS NOT NULL,
    floor(extract(epoch FROM r.mail_hour))::int8, r.mail_in_hour,
    (r.mail_day - DATE '1970-01-01')::int8, r.mail_in_day,
    (SELECT s.doc::text FROM rule_state s WHERE s.rule_id = r.id AND s.scope = p.id),
    (SELECT s.doc::text FROM rule_state s WHERE s.rule_id = r.id AND s.scope = ''),
    (SELECT json_agg(json_build_array(a.scope = '', a.key, a.firing,
        floor(extract(epoch FROM a.decided_at))::int8,
        floor(extract(epoch FROM a.changed_at))::int8,
        floor(extract(epoch FROM a.last_sent_at))::int8))::text
      FROM rule_alert_state a WHERE a.rule_id = r.id AND a.scope IN ('', p.id))
  FROM pigeons p
  JOIN flocks f ON f.id = p.flock_id
  LEFT JOIN rules r ON r.flock_id = f.id AND r.enabled AND r.blocked_at IS NULL
    AND r.deployed_version IS NOT NULL
  LEFT JOIN rule_bundles b ON b.flock_id = f.id
  WHERE p.id = $1 AND p.suspended_at IS NULL
  ORDER BY r.created_at, r.id";

/// One flag of [`FRESH_READ_SQL`]'s array: flock scope, key, firing, decided, changed, last sent.
type FlagRow = (bool, String, bool, i64, i64, Option<i64>);

fn state_doc(text: Option<String>) -> Map<String, Value> {
  text
    .and_then(|t| serde_json::from_str(&t).ok())
    .unwrap_or_default()
}

fn flags(text: Option<String>) -> AlertFlags {
  let rows: Vec<FlagRow> = text
    .and_then(|t| serde_json::from_str(&t).ok())
    .unwrap_or_default();
  rows
    .into_iter()
    .map(
      |(flock, key, firing, decided_at, changed_at, last_sent_at)| {
        let scope = if flock {
          RuleAlertScope::Flock
        } else {
          RuleAlertScope::Pigeon
        };
        let flag = AlertFlag {
          firing,
          decided_at,
          changed_at,
          last_sent_at,
        };
        ((scope, key), flag)
      },
    )
    .collect()
}

/// What the second pass dispatches for `pigeon_id`: `None` when the pigeon is gone or suspended.
pub async fn fresh_read(
  client: &Client,
  pigeon_id: &str,
) -> std::result::Result<Option<Lookup>, tokio_postgres::Error> {
  let rows = client
    .query_typed(FRESH_READ_SQL, &[(&pigeon_id, Type::TEXT)])
    .await?;
  let Some(first) = rows.first() else {
    return Ok(None);
  };
  let flock: Uuid = first.get(1);
  let mut lookup = Lookup {
    now: first.get(0),
    flock: RuleNamedRef {
      id: flock.to_string(),
      name: first.get(2),
    },
    pigeon: RuleNamedRef {
      id: pigeon_id.to_string(),
      name: first.get(3),
    },
    account: first.get(4),
    org: first.get(5),
    uploaded_at: first.get(6),
    leased: first.get(7),
    account_mail_day: first.get(8),
    platform_mail_day: first.get(9),
    rules: Vec::with_capacity(rows.len()),
  };
  for row in &rows {
    let Some(id) = row.get::<_, Option<Uuid>>(10) else {
      continue;
    };
    lookup.rules.push(LiveRule {
      id,
      version: row.get(11),
      failing: row.get(12),
      watched: row.get(13),
      pending: row.get(14),
      mail: RuleMailCounters {
        hour_start: row.get(15),
        in_hour: row.get(16),
        day: row.get(17),
        in_day: row.get(18),
      },
      state: RuleState {
        pigeon: state_doc(row.get(19)),
        flock: state_doc(row.get(20)),
      },
      flags: flags(row.get(21)),
    });
  }
  Ok(Some(lookup))
}

/// Locks the rule's row, as [`lock_rule`] does, and reads its streak under the lock, so two
/// batches' failures both count; and whether the rule is still enabled, unblocked and deployed at
/// the version dispatched, since a promote, a disable or a block may have landed since the read.
const LOCK_STREAK_SQL: &str = "SELECT fail_count, floor(extract(epoch FROM fail_since))::int8,
    enabled AND blocked_at IS NULL AND deployed_version IS NOT DISTINCT FROM $2
  FROM rules WHERE id = $1 FOR UPDATE";

/// Writes a streak, and with `$4` set disables the rule with that `paused_reason`.
const STREAK_SQL: &str = "UPDATE rules SET fail_count = $2, fail_since = to_timestamp($3),
    enabled = enabled AND $4::text IS NULL, paused_reason = COALESCE($4::text, paused_reason),
    updated_at = CASE WHEN $4::text IS NULL THEN updated_at ELSE now() END
  WHERE id = $1";

/// A result's values, one array per column.
const VALUES_SQL: &str = "INSERT INTO rule_values (rule_id, pigeon_id, key, value, value_num, at)
  SELECT $1, $2, v.key, v.value, v.num, to_timestamp(v.at)
  FROM unnest($3::text[], $4::text[], $5::float8[], $6::int8[]) AS v(key, value, num, at)
  ON CONFLICT DO NOTHING";

/// Keyed on the batch, so a redelivered batch keeps the first delivery's row.
const RUN_SQL: &str = "INSERT INTO rule_runs (rule_id, version, pigeon_id, batch_id, status,
    readings, alerts_sent, alerts_suppressed, values_written, state_changed, wall_ms, error, logs)
  VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::jsonb)
  ON CONFLICT DO NOTHING";

/// A rule mail claimed under the caps, to send once its transaction has committed.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleMail {
  /// The rule that decided it.
  pub rule_id: Uuid,
  /// What the mail is.
  pub kind: RuleMailKind,
}

/// What a committed run mails.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleMailKind {
  /// One of the rule's own alerts, to its recipients.
  Alert(RuleAlert),
  /// The run that paused the rule, to its author once: `auto:<status>` and the run's error.
  Paused {
    /// The `paused_reason` written.
    reason: String,
    /// The pausing run's error, the rule's own text.
    error: Option<String>,
  },
}

/// Records every plan that writes, one transaction per rule, and returns the mails the committed
/// ones claimed. A rule whose transaction loses a race writes nothing and the rest go on; one
/// whose writes Postgres refuses as data records [`RulePlan::unstorable`] instead. Any other
/// error stops and comes back beside the mails already claimed, which are still the caller's to
/// send before it skips the rest of its pass.
pub async fn record_runs(
  client: &mut Client,
  lookup: &Lookup,
  work: &RuleWork,
  plans: &[RulePlan],
  wall_ms: i32,
) -> (Vec<RuleMail>, Option<tokio_postgres::Error>) {
  let last = work
    .readings
    .iter()
    .map(|r| r.at_secs)
    .max()
    .unwrap_or(lookup.now);
  let mut mails = Vec::new();
  for plan in plans.iter().filter(|plan| plan.writes) {
    let recorded = match record_run(client, lookup, work, plan, last, wall_ms).await {
      Err(e) if is_data_error(&e) => {
        let code = e.code().map_or("none", |code| code.code());
        log_line(&["rules: Postgres refused a run's writes, SQLSTATE ", code]);
        record_run(client, lookup, work, &plan.unstorable(), last, wall_ms).await
      }
      recorded => recorded,
    };
    match recorded {
      Ok(mut claimed) => mails.append(&mut claimed),
      Err(e) if is_contention(&e) => log_line(&["rules: a run's record lost a race"]),
      Err(e) => return (mails, Some(e)),
    }
  }
  (mails, None)
}

/// Logs `parts` as one line, from code the host tests run too, where the Workers console does not
/// exist.
fn log_line(parts: &[&str]) {
  let mut line = String::with_capacity(parts.iter().map(|part| part.len()).sum());
  for part in parts {
    line.push_str(part);
  }
  #[cfg(target_arch = "wasm32")]
  console_error!("{line}");
  #[cfg(not(target_arch = "wasm32"))]
  eprintln!("{line}");
}

/// A data exception or an integrity violation (SQLSTATE classes 22 and 23): the row, not the
/// database, so it is that rule's to answer for rather than every later tenant's.
fn is_data_error(error: &tokio_postgres::Error) -> bool {
  error
    .code()
    .is_some_and(|code| matches!(code.code().get(..2), Some("22" | "23")))
}

/// One plan's writes in one transaction, the streak read under the row lock. A rule deleted,
/// promoted, disabled or blocked since the read writes nothing.
async fn record_run(
  client: &mut Client,
  lookup: &Lookup,
  work: &RuleWork,
  plan: &RulePlan,
  last_reading: i64,
  wall_ms: i32,
) -> std::result::Result<Vec<RuleMail>, tokio_postgres::Error> {
  let id = plan.rule_id;
  let tx = begin(client).await?;
  let body = async {
    let Some(row) = tx
      .query_typed_opt(
        LOCK_STREAK_SQL,
        &[(&id, Type::UUID), (&plan.version, Type::INT4)],
      )
      .await?
    else {
      return Ok(Vec::new());
    };
    if !row.get::<_, bool>(2) {
      return Ok(Vec::new());
    }
    let streak = FailStreak {
      count: row.get(0),
      since: row.get(1),
    };
    let mut mails = Vec::new();
    let (mut sent, mut suppressed, mut written, mut changed) = (0i32, 0i32, 0i32, false);
    if let Some(apply) = &plan.apply {
      if let Some(patch) = &apply.pigeon_patch {
        changed |= merge_state_row(&tx, id, &work.pigeon_id, patch).await?;
      }
      if let Some(patch) = &apply.flock_patch {
        changed |= merge_state_row(&tx, id, "", patch).await?;
      }
      written = insert_values(&tx, id, &work.pigeon_id, &apply.values).await?;
      for decision in &apply.alerts {
        let alert = &decision.alert;
        let scope = scope_key(alert.scope, &work.pigeon_id);
        if !apply_alert(
          &tx,
          id,
          scope,
          &alert.key,
          alert.kind,
          last_reading,
          lookup.now,
        )
        .await?
        {
          continue;
        }
        if decision.mailed && claim_mail(&tx, id, lookup.account, lookup.now).await? {
          sent += 1;
          mails.push(RuleMail {
            rule_id: id,
            kind: RuleMailKind::Alert(alert.clone()),
          });
        } else {
          suppressed += 1;
        }
      }
    }
    if plan.run_row {
      let logs = Value::from(plan.logs.clone()).to_string();
      let readings = i32::try_from(work.readings.len()).unwrap_or(i32::MAX);
      tx.query_typed(
        RUN_SQL,
        &[
          (&id, Type::UUID),
          (&plan.version, Type::INT4),
          (&work.pigeon_id, Type::TEXT),
          (&work.batch_id, Type::TEXT),
          (&plan.status.as_str(), Type::TEXT),
          (&readings, Type::INT4),
          (&sent, Type::INT4),
          (&suppressed, Type::INT4),
          (&written, Type::INT4),
          (&changed, Type::BOOL),
          (&wall_ms, Type::INT4),
          (&plan.error, Type::TEXT),
          (&logs, Type::TEXT),
        ],
      )
      .await?;
    }
    let (next, reason) = match next_streak(streak, plan.status, lookup.now) {
      StreakWrite::Keep => (None, None),
      StreakWrite::Reset => (Some(FailStreak::default()), None),
      StreakWrite::Set(next) => (Some(next), None),
      StreakWrite::Pause { streak, reason } => {
        mails.push(RuleMail {
          rule_id: id,
          kind: RuleMailKind::Paused {
            reason: reason.clone(),
            error: plan.error.clone(),
          },
        });
        (Some(streak), Some(reason))
      }
    };
    if let Some(next) = next {
      tx.query_typed(
        STREAK_SQL,
        &[
          (&id, Type::UUID),
          (&next.count, Type::INT4),
          (&next.since, Type::INT8),
          (&reason, Type::TEXT),
        ],
      )
      .await?;
    }
    Ok(mails)
  }
  .await;
  end(tx, body, |e| e).await
}

/// Who a rule's mail may go to, read only when a run claimed one: the rule's name and list, and
/// the addresses the account holds, as alert mail rechecks them at send.
const MAIL_TARGETS_SQL: &str = "SELECT r.id, r.name, r.recipients::text, f.owner_email,
    (SELECT string_agg(DISTINCT m.email, ',') FROM organization_members m
      WHERE m.org_id = f.org_id AND m.email IS NOT NULL),
    (SELECT m.email FROM organization_members m
      WHERE m.org_id = f.org_id AND m.user_id = r.created_by)
  FROM rules r JOIN flocks f ON f.id = r.flock_id WHERE r.id = ANY($1)";

/// One rule's [`MAIL_TARGETS_SQL`] row.
#[derive(Debug, Clone, PartialEq)]
pub struct MailTarget {
  /// The rule.
  pub rule_id: Uuid,
  /// Its name, for the mail's footer.
  pub name: String,
  /// Its own recipient list.
  pub recipients: Vec<String>,
  /// The flock owner's address.
  pub owner_email: Option<String>,
  /// The owning organization's member addresses, comma-separated.
  pub member_emails: Option<String>,
  /// The rule's author's address, while the author is a member of the owning organization.
  pub author_email: Option<String>,
}

/// [`MailTarget`]s of `rules`.
pub async fn mail_targets(
  client: &Client,
  rules: &[Uuid],
) -> std::result::Result<Vec<MailTarget>, tokio_postgres::Error> {
  let rows = client
    .query_typed(MAIL_TARGETS_SQL, &[(&rules, Type::UUID_ARRAY)])
    .await?;
  Ok(
    rows
      .iter()
      .map(|row| MailTarget {
        rule_id: row.get(0),
        name: row.get(1),
        recipients: serde_json::from_str(row.get(2)).unwrap_or_default(),
        owner_email: row.get(3),
        member_emails: row.get(4),
        author_email: row.get(5),
      })
      .collect(),
  )
}

/// Inserts a result's values, a value already stored at its key and time kept as it was. Returns
/// how many were new.
async fn insert_values(
  tx: &Transaction<'_>,
  rule_id: Uuid,
  pigeon_id: &str,
  values: &[PlannedValue],
) -> std::result::Result<i32, tokio_postgres::Error> {
  if values.is_empty() {
    return Ok(0);
  }
  let keys: Vec<&str> = values.iter().map(|v| v.key.as_str()).collect();
  let texts: Vec<&str> = values.iter().map(|v| v.value.as_str()).collect();
  let nums: Vec<Option<f64>> = values.iter().map(|v| v.value_num).collect();
  let ats: Vec<i64> = values.iter().map(|v| v.at).collect();
  let rows = tx
    .execute_typed(
      VALUES_SQL,
      &[
        (&rule_id, Type::UUID),
        (&pigeon_id, Type::TEXT),
        (&keys, Type::TEXT_ARRAY),
        (&texts, Type::TEXT_ARRAY),
        (&nums, Type::FLOAT8_ARRAY),
        (&ats, Type::INT8_ARRAY),
      ],
    )
    .await?;
  Ok(i32::try_from(rows).unwrap_or(i32::MAX))
}

/// How long a deploy holds its flock's lease; the upload it guards is aborted at 30 s.
pub const DEPLOY_LEASE_SECS: i64 = 60;

/// Takes the lease, creating the flock's bundle row with the contract and tag it will be uploaded
/// at when there is none. One upsert, so of two concurrent deploys exactly one gets the row.
const TAKE_LEASE_SQL: &str = "INSERT INTO rule_bundles AS b
    (flock_id, contract, account_tag, deploy_until)
  VALUES ($1, $2, $3, now() + make_interval(secs => $4))
  ON CONFLICT (flock_id) DO UPDATE SET deploy_until = EXCLUDED.deploy_until
  WHERE b.deploy_until < now()
  RETURNING deploy_until";

/// Frees the lease only while it is still the one taken, so a deploy that overran it never
/// frees the next deploy's.
const RELEASE_LEASE_SQL: &str = "UPDATE rule_bundles SET deploy_until = 'epoch'
  WHERE flock_id = $1 AND deploy_until = $2
  RETURNING 1";

/// Takes `flock_id`'s deploy lease for [`DEPLOY_LEASE_SECS`]. `None` means a deploy is in
/// progress (the route's 409); `Some` carries the lease to hand back to
/// [`release_deploy_lease`]. A lease rather than an advisory lock, whose session behind
/// Hyperdrive is not something to rely on. A first deploy creates the bundle row before its
/// upload, so `uploaded_at IS NULL` is what means the flock has no bundle.
pub async fn take_deploy_lease(
  client: &Client,
  flock_id: Uuid,
  contract: i32,
  account_tag: &str,
) -> std::result::Result<Option<OffsetDateTime>, tokio_postgres::Error> {
  let secs = DEPLOY_LEASE_SECS as f64;
  let rows = client
    .query_typed(
      TAKE_LEASE_SQL,
      &[
        (&flock_id, Type::UUID),
        (&contract, Type::INT4),
        (&account_tag, Type::TEXT),
        (&secs, Type::FLOAT8),
      ],
    )
    .await?;
  Ok(rows.first().map(|row| row.get(0)))
}

/// Releases the lease [`take_deploy_lease`] returned. False when it had already expired and
/// another deploy took it.
pub async fn release_deploy_lease(
  client: &Client,
  flock_id: Uuid,
  lease: OffsetDateTime,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let rows = client
    .query_typed(
      RELEASE_LEASE_SQL,
      &[(&flock_id, Type::UUID), (&lease, Type::TIMESTAMPTZ)],
    )
    .await?;
  Ok(!rows.is_empty())
}

/// What one assistant call cost, as the model that served it reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssistantUsage {
  /// Uncached input tokens.
  pub input_tokens: i64,
  /// Output tokens.
  pub output_tokens: i64,
  /// Input tokens read from the prompt cache.
  pub cache_read_tokens: i64,
  /// Input tokens written to the prompt cache.
  pub cache_write_tokens: i64,
  /// The call's price in millionths of a dollar.
  pub cost_micro_usd: i64,
}

/// Adds one turn to the account's and the platform's rows for the UTC day of `$2`.
const ASSISTANT_TURN_SQL: &str = "INSERT INTO rule_usage AS u (scope, day, assistant_turns,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_micro_usd)
  SELECT s, (to_timestamp($2) AT TIME ZONE 'UTC')::date, 1, $3, $4, $5, $6, $7
  FROM unnest(ARRAY[$1::text, 'platform']) AS s
  ON CONFLICT (scope, day) DO UPDATE SET
    assistant_turns = u.assistant_turns + 1,
    input_tokens = u.input_tokens + EXCLUDED.input_tokens,
    output_tokens = u.output_tokens + EXCLUDED.output_tokens,
    cache_read_tokens = u.cache_read_tokens + EXCLUDED.cache_read_tokens,
    cache_write_tokens = u.cache_write_tokens + EXCLUDED.cache_write_tokens,
    cost_micro_usd = u.cost_micro_usd + EXCLUDED.cost_micro_usd";

/// Records one assistant call at `now` against `account` (the flock owner's organization id,
/// else its user id) and the platform. Every call counts, refused and explain-back ones included,
/// and nothing is ever taken back, so deleting a rule refunds no allowance.
pub async fn record_assistant_turn(
  client: &Client,
  account: Uuid,
  now: i64,
  usage: &AssistantUsage,
) -> std::result::Result<(), tokio_postgres::Error> {
  client
    .query_typed(
      ASSISTANT_TURN_SQL,
      &[
        (&account, Type::UUID),
        (&now, Type::INT8),
        (&usage.input_tokens, Type::INT8),
        (&usage.output_tokens, Type::INT8),
        (&usage.cache_read_tokens, Type::INT8),
        (&usage.cache_write_tokens, Type::INT8),
        (&usage.cost_micro_usd, Type::INT8),
      ],
    )
    .await?;
  Ok(())
}

/// The flock's history since `since` for a rule's dry run, never reading a row under a pigeon
/// [`excluded_pigeons`] names for `excluded_var` (`RULES_EXCLUDED_PIGEONS`). The only way rule
/// code reads history, so no caller can forget the exclusion.
pub async fn replay_history(
  client: &Client,
  access: &FlockAccess,
  since: OffsetDateTime,
  excluded_var: Option<&str>,
) -> Result<TelemetryHistoryPage> {
  let exclude = excluded_pigeons(excluded_var);
  query_telemetry_history_for_flock(client, access, None, Some(since), None, &exclude).await
}

/// Deletes what rules stored under a deleted pigeon, best effort: its state documents and alert
/// flags key on the pigeon's id as text, with no foreign key to cascade, and a NIDD pigeon's id
/// comes back when its IMEI is created again. Nothing is done under an excluded pigeon, and where
/// the rule tables do not exist nothing is logged.
pub async fn delete_pigeon_rule_rows(client: &Client, pigeon_id: &str) {
  if excluded_pigeons(None)
    .iter()
    .any(|id| id.eq_ignore_ascii_case(pigeon_id))
  {
    return;
  }
  let deleted = client
    .execute_typed(
      "WITH s AS (DELETE FROM rule_state WHERE scope = $1)
       DELETE FROM rule_alert_state WHERE scope = $1",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await;
  if let Err(e) = deleted
    && e.code() != Some(&SqlState::UNDEFINED_TABLE)
  {
    let code = e.code().map_or("none", |code| code.code());
    console_error!("rules: a deleted pigeon's rule rows were not deleted, SQLSTATE {code}");
  }
}

/// The flock's name and its pigeons' names for a dry run, never naming a pigeon
/// [`excluded_pigeons`] names; `None` when the flock is gone.
pub async fn replay_names(
  client: &Client,
  access: &FlockAccess,
  excluded_var: Option<&str>,
) -> std::result::Result<Option<(String, HashMap<String, String>)>, tokio_postgres::Error> {
  let Ok(flock) = Uuid::parse_str(access.flock_id()) else {
    return Ok(None);
  };
  let exclude = excluded_pigeons(excluded_var);
  let rows = client
    .query_typed(
      "SELECT f.name, p.id, COALESCE(p.name, '') FROM flocks f
       LEFT JOIN pigeons p ON p.flock_id = f.id AND p.id <> ALL($2)
       WHERE f.id = $1",
      &[(&flock, Type::UUID), (&exclude, Type::TEXT_ARRAY)],
    )
    .await?;
  let Some(first) = rows.first() else {
    return Ok(None);
  };
  let names = rows
    .iter()
    .filter_map(|row| Some((row.get::<_, Option<String>>(1)?, row.get(2))))
    .collect();
  Ok(Some((first.get(0), names)))
}

#[cfg(test)]
mod tests {
  use super::*;

  const MIGRATION: &str = include_str!("../../../infra/migrations/2026-11-03-rules.sql");
  const BUILDER_MIGRATION: &str =
    include_str!("../../../infra/migrations/2026-11-07-rule-builder.sql");
  const INIT_DB: &str = include_str!("../../../infra/init-db.sql");

  #[test]
  fn the_migration_and_init_db_carry_the_ensured_tables_verbatim() {
    assert!(MIGRATION.contains(RULE_TABLES_DDL));
    assert!(INIT_DB.contains(RULE_TABLES_DDL));
    assert!(BUILDER_MIGRATION.contains(RULE_BUILDER_DDL));
    let tables = INIT_DB
      .find(RULE_TABLES_DDL)
      .expect("tables in init-db.sql");
    let builder = INIT_DB
      .find(RULE_BUILDER_DDL)
      .expect("builder in init-db.sql");
    assert!(
      builder > tables,
      "the builder's ALTER runs after the tables exist"
    );
  }

  #[test]
  fn every_ensured_relation_is_created_by_the_ddl() {
    for name in RULE_RELATIONS {
      let mut table = String::with_capacity(29 + name.len());
      table.push_str("CREATE TABLE IF NOT EXISTS ");
      table.push_str(name);
      table.push_str(" (");
      let mut index = String::with_capacity(31 + name.len());
      index.push_str("CREATE INDEX IF NOT EXISTS ");
      index.push_str(name);
      index.push_str(" ON ");
      assert!(
        RULE_TABLES_DDL.contains(&table) || RULE_TABLES_DDL.contains(&index),
        "an ensured relation is not created"
      );
    }
  }
}

/// Tests against a real Postgres: the dev docker service (`infra/docker-compose.yml`), reached
/// through `DOVECOTE_TEST_PG`, a libpq key=value string for a superuser and its maintenance
/// database. Each test makes and drops its own scratch database owned by `dovecote`, built from
/// `infra/init-db.sql` and the rules migration, and runs as `dovecote` like the Worker does.
///
/// `cargo test -p dovecote --target x86_64-unknown-linux-gnu -- --ignored rule_store`
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) mod pg {
  use super::*;
  use std::collections::VecDeque;
  use std::future::Future;
  use std::sync::atomic::AtomicUsize;
  use std::sync::{Arc, Mutex};
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  use tokio::net::tcp::OwnedReadHalf;
  use tokio::net::{TcpListener, TcpStream};
  use tokio_postgres::config::Host;
  use tokio_postgres::{Config, NoTls};

  use crate::helpers::ResolvedReading;
  use crate::helpers::rules::{
    AUTO_PAUSE_RUNS, Classified, DispatchOutcome, DispatchRule, RunStatus, classify, plan_record,
    uuid_str,
  };

  const MIGRATION: &str = include_str!("../../../infra/migrations/2026-11-03-rules.sql");
  const INIT_DB: &str = include_str!("../../../infra/init-db.sql");

  /// Runs one async test body on a single-threaded runtime.
  pub fn run<F: Future<Output = ()>>(body: F) {
    tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()
      .expect("test runtime builds")
      .block_on(body);
  }

  fn config() -> Config {
    std::env::var("DOVECOTE_TEST_PG")
      .expect("DOVECOTE_TEST_PG names the dev Postgres")
      .parse()
      .expect("DOVECOTE_TEST_PG parses as a connection string")
  }

  /// A connection to `dbname`, its driver spawned on the current runtime.
  pub async fn connect(dbname: Option<&str>) -> Client {
    let mut config = config();
    if let Some(dbname) = dbname {
      config.dbname(dbname);
    }
    let Some(Host::Tcp(host)) = config.get_hosts().first().cloned() else {
      panic!("DOVECOTE_TEST_PG names no TCP host");
    };
    let port = config.get_ports().first().copied().unwrap_or(5432);
    let stream = TcpStream::connect((host.as_str(), port))
      .await
      .expect("dev Postgres reachable");
    let (client, connection) = config
      .connect_raw(stream, NoTls)
      .await
      .expect("dev Postgres accepts the connection");
    tokio::spawn(connection);
    client
  }

  /// A scratch database for one test.
  pub struct Scratch {
    name: String,
  }

  impl Scratch {
    /// Creates `rules_t_<tag>` afresh, with the schema a migrated production database has.
    pub async fn new(tag: &str) -> Self {
      let mut name = String::with_capacity(8 + tag.len());
      name.push_str("rules_t_");
      name.push_str(tag);
      let admin = connect(None).await;
      let scratch = Self { name };
      scratch.drop_with(&admin).await;
      let mut sql = String::with_capacity(32 + scratch.name.len());
      sql.push_str("CREATE DATABASE ");
      sql.push_str(&scratch.name);
      sql.push_str(" OWNER dovecote;");
      admin
        .batch_execute(&sql)
        .await
        .expect("scratch database created");
      let client = connect(Some(&scratch.name)).await;
      let (_, body) = INIT_DB
        .split_once("\\c dovecote;")
        .expect("init-db.sql connects to dovecote");
      client
        .batch_execute("SET ROLE dovecote;")
        .await
        .expect("role");
      client
        .batch_execute(body)
        .await
        .expect("init-db.sql applies");
      client
        .batch_execute(MIGRATION)
        .await
        .expect("migration applies");
      scratch
    }

    /// A connection running as `dovecote`.
    pub async fn client(&self) -> Client {
      let client = connect(Some(&self.name)).await;
      client
        .batch_execute("SET ROLE dovecote;")
        .await
        .expect("role");
      client
    }

    /// Drops the database. A test that panics first leaves it for the next run's `new`.
    pub async fn drop(self) {
      self.drop_with(&connect(None).await).await;
    }

    async fn drop_with(&self, admin: &Client) {
      let mut sql = String::with_capacity(40 + self.name.len());
      sql.push_str("DROP DATABASE IF EXISTS ");
      sql.push_str(&self.name);
      sql.push_str(" WITH (FORCE);");
      admin
        .batch_execute(&sql)
        .await
        .expect("scratch database dropped");
    }
  }

  /// Polls until backend `pid` waits on a lock, so a test knows its second caller really is
  /// blocked behind the first rather than merely late. Observes as the superuser: a role that is
  /// not the session's own sees no wait event.
  pub async fn wait_until_blocked(pid: i32) {
    let observer = connect(None).await;
    for _ in 0..500 {
      let rows = observer
        .query(
          "SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND wait_event_type = 'Lock'",
          &[&pid],
        )
        .await
        .expect("pg_stat_activity readable");
      if !rows.is_empty() {
        return;
      }
      tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the second caller never waited on a lock");
  }

  /// This connection's backend pid.
  pub async fn pid(client: &Client) -> i32 {
    client
      .query_one("SELECT pg_backend_pid()", &[])
      .await
      .expect("backend pid")
      .get(0)
  }

  /// One protocol message off `half` as raw bytes, with its tag (0 for the untagged startup
  /// message); `None` once the peer closes.
  async fn wire_message(half: &mut OwnedReadHalf, tagged: bool) -> Option<(u8, Vec<u8>)> {
    let mut raw = Vec::with_capacity(5);
    let mut tag = 0;
    if tagged {
      tag = half.read_u8().await.ok()?;
      raw.push(tag);
    }
    let len = half.read_u32().await.ok()?;
    raw.extend_from_slice(&len.to_be_bytes());
    let start = raw.len();
    raw.resize(start + len as usize - 4, 0);
    half.read_exact(&mut raw[start..]).await.ok()?;
    Some((tag, raw))
  }

  /// A connection to `db` through a proxy that holds each `ReadyForQuery` back 20 ms and counts
  /// every message the client writes while a simple query's reply is still outstanding. Every
  /// simple query the code awaits is answered first, so a count means a dropped `Transaction`'s
  /// `ROLLBACK` with a request pipelined behind it: what Hyperdrive answered with silence.
  pub async fn pipeline_watched(db: &Scratch) -> (Client, Arc<AtomicUsize>) {
    let mut config = config();
    let Some(Host::Tcp(host)) = config.get_hosts().first().cloned() else {
      panic!("DOVECOTE_TEST_PG names no TCP host");
    };
    let port = config.get_ports().first().copied().unwrap_or(5432);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy binds");
    let local = listener.local_addr().expect("proxy address").port();
    let piped = Arc::new(AtomicUsize::new(0));
    let counted = piped.clone();
    tokio::spawn(async move {
      let (front, _) = listener.accept().await.expect("client reaches the proxy");
      let back = TcpStream::connect((host.as_str(), port))
        .await
        .expect("dev Postgres reachable");
      let (mut client_read, mut client_write) = front.into_split();
      let (mut server_read, mut server_write) = back.into_split();
      // Each request awaiting its ReadyForQuery, true for a simple query.
      let waiting = Arc::new(Mutex::new(VecDeque::<bool>::new()));
      let ready = Arc::new(AtomicBool::new(false));
      let (up_waiting, up_ready) = (waiting.clone(), ready.clone());
      tokio::spawn(async move {
        let mut tagged = false;
        while let Some((tag, raw)) = wire_message(&mut client_read, tagged).await {
          tagged = true;
          if up_ready.load(Ordering::SeqCst) && tag != b'X' {
            let mut waiting = up_waiting.lock().expect("proxy state");
            if waiting.iter().any(|simple| *simple) {
              counted.fetch_add(1, Ordering::SeqCst);
            }
            match tag {
              b'Q' => waiting.push_back(true),
              b'S' => waiting.push_back(false),
              _ => {}
            }
          }
          if server_write.write_all(&raw).await.is_err() {
            return;
          }
        }
      });
      while let Some((tag, raw)) = wire_message(&mut server_read, true).await {
        if tag == b'Z' && ready.swap(true, Ordering::SeqCst) {
          tokio::time::sleep(std::time::Duration::from_millis(20)).await;
          waiting.lock().expect("proxy state").pop_front();
        }
        if client_write.write_all(&raw).await.is_err() {
          return;
        }
      }
    });
    config.dbname(&db.name);
    let stream = TcpStream::connect(("127.0.0.1", local))
      .await
      .expect("proxy reachable");
    let (client, connection) = config
      .connect_raw(stream, NoTls)
      .await
      .expect("dev Postgres accepts the proxied connection");
    tokio::spawn(connection);
    client
      .batch_execute("SET ROLE dovecote;")
      .await
      .expect("role");
    (client, piped)
  }

  /// A flock owned by a fresh user, or by `org` when given.
  pub async fn new_flock(client: &Client, org: Option<Uuid>) -> Uuid {
    client
      .query_one(
        "INSERT INTO flocks (user_id, name, org_id) VALUES (gen_random_uuid(), 'f', $1)
         RETURNING id",
        &[&org],
      )
      .await
      .expect("flock inserted")
      .get(0)
  }

  /// A disabled rule in `flock`.
  pub async fn new_rule(client: &Client, flock: Uuid) -> Uuid {
    client
      .query_one(
        "INSERT INTO rules (flock_id, created_by, name) VALUES ($1, gen_random_uuid(), 'r')
         RETURNING id",
        &[&flock],
      )
      .await
      .expect("rule inserted")
      .get(0)
  }

  /// A pigeon in `flock` under `id`.
  pub async fn new_pigeon(client: &Client, flock: Uuid, id: &str) {
    client
      .execute(
        "INSERT INTO pigeons (id, flock_id, connector, created_at, updated_at)
         VALUES ($1, $2, '{}', now(), now())",
        &[&id, &flock],
      )
      .await
      .expect("pigeon inserted");
  }

  fn obj(value: serde_json::Value) -> Map<String, Value> {
    match value {
      Value::Object(map) => map,
      _ => panic!("a test patch is an object"),
    }
  }

  async fn doc(client: &Client, rule: Uuid, scope: &str) -> Value {
    let text: String = client
      .query_one(
        "SELECT doc::text FROM rule_state WHERE rule_id = $1 AND scope = $2",
        &[&rule, &scope],
      )
      .await
      .expect("state row")
      .get(0);
    serde_json::from_str(&text).expect("state is JSON")
  }

  async fn mail_sent(client: &Client, scope: &str, now: i64) -> i32 {
    client
      .query_opt(
        "SELECT mail_sent FROM rule_usage
         WHERE scope = $1 AND day = (to_timestamp($2::bigint) AT TIME ZONE 'UTC')::date",
        &[&scope, &now],
      )
      .await
      .expect("usage readable")
      .map_or(0, |row| row.get(0))
  }

  /// Claims `n` mails for one rule at `now`, each in its own transaction, and counts the grants.
  async fn claims(client: &mut Client, rule: Uuid, account: Uuid, now: i64, n: usize) -> usize {
    let mut granted = 0;
    for _ in 0..n {
      let tx = client.transaction().await.expect("tx");
      if claim_mail(&tx, rule, account, now).await.expect("claim") {
        granted += 1;
      }
      tx.commit().await.expect("commit");
    }
    granted
  }

  /// A unix time at the top of a UTC day, far from any real clock.
  const DAY: i64 = 1_900_000_000 - 1_900_000_000 % 86_400;

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_the_migration_alone_creates_every_rule_table() {
    run(async {
      let db = Scratch::new("schema").await;
      let client = db.client().await;
      assert!(
        missing_schema_objects(&client, &RULE_RELATIONS, &[])
          .await
          .expect("catalog readable")
          .is_empty()
      );
      client
        .batch_execute(
          "DROP TABLE rule_usage, rule_bundles, rule_runs, rule_values, rule_alert_state,
             rule_state, rule_versions, rules;",
        )
        .await
        .expect("rule tables dropped");
      assert_eq!(
        missing_schema_objects(&client, &RULE_RELATIONS, &[])
          .await
          .expect("catalog readable")
          .len(),
        RULE_RELATIONS.len()
      );
      client
        .batch_execute(MIGRATION)
        .await
        .expect("migration applies");
      client
        .batch_execute(MIGRATION)
        .await
        .expect("migration reapplies");
      // The migration ends in RESET ROLE; ensure runs as the Worker's role.
      client
        .batch_execute("SET ROLE dovecote;")
        .await
        .expect("role");
      client
        .batch_execute(RULE_TABLES_DDL)
        .await
        .expect("ensure reapplies");
      ensure_rule_tables(&client)
        .await
        .expect("ensure finds them");
      assert!(
        missing_schema_objects(&client, &RULE_RELATIONS, &[])
          .await
          .expect("catalog readable")
          .is_empty()
      );
      drop(client);
      db.drop().await;
    });
  }
  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_the_builder_migration_widens_a_schema_from_before_it() {
    const BUILDER_MIGRATION: &str =
      include_str!("../../../infra/migrations/2026-11-07-rule-builder.sql");
    run(async {
      let db = Scratch::new("builder_schema").await;
      let client = db.client().await;
      let rule = new_rule(&client, new_flock(&client, None).await).await;
      let insert = "INSERT INTO rule_versions (rule_id, version, source, sha256, contract, origin,
          created_by, builder_model)
        VALUES ($1, $2, 'x', 'aa', 1, 'builder', $1, '{}'::jsonb)";
      client
        .execute(insert, &[&rule, &1i32])
        .await
        .expect("a fresh schema takes a builder version");
      // The schema staging holds from before the builder: the narrow CHECK, no column.
      client
        .batch_execute(
          "DELETE FROM rule_versions;
           ALTER TABLE rule_versions DROP COLUMN builder_model;
           ALTER TABLE rule_versions DROP CONSTRAINT rule_versions_origin_check;
           ALTER TABLE rule_versions ADD CONSTRAINT rule_versions_origin_check
             CHECK (origin IN ('hand', 'assistant', 'assistant_edited'));",
        )
        .await
        .expect("narrowed");
      assert_eq!(
        missing_schema_objects(&client, &RULE_RELATIONS, &RULE_COLUMNS)
          .await
          .expect("catalog readable"),
        ["rule_versions.builder_model"]
      );
      // What ensure runs on finding the column missing; it logs first, which needs workerd.
      client
        .batch_execute(RULE_BUILDER_DDL)
        .await
        .expect("the ensure DDL widens it");
      assert!(
        missing_schema_objects(&client, &RULE_RELATIONS, &RULE_COLUMNS)
          .await
          .expect("catalog readable")
          .is_empty()
      );
      client
        .execute(insert, &[&rule, &2i32])
        .await
        .expect("the widened schema takes a builder version");
      let refused = client
        .execute(&insert.replace("'builder'", "'robot'"), &[&rule, &3i32])
        .await
        .expect_err("an unknown origin is still refused");
      assert_eq!(refused.code(), Some(&SqlState::CHECK_VIOLATION));
      client
        .batch_execute(BUILDER_MIGRATION)
        .await
        .expect("the migration reapplies");
      client
        .batch_execute(BUILDER_MIGRATION)
        .await
        .expect("and again");
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_deleted_pigeon_leaves_no_rule_rows_and_a_missing_table_is_quiet() {
    run(async {
      let db = Scratch::new("pigeon_rows").await;
      let client = db.client().await;
      let rule = new_rule(&client, new_flock(&client, None).await).await;
      let field = crate::helpers::rules::FIELD_PIGEONS[0];
      for scope in ["p1", "p2", "", field] {
        client
          .execute(
            "INSERT INTO rule_state (rule_id, scope, doc) VALUES ($1, $2, '{}')",
            &[&rule, &scope],
          )
          .await
          .expect("state");
        client
          .execute(
            "INSERT INTO rule_alert_state (rule_id, scope, key, firing, decided_at)
             VALUES ($1, $2, 'k', true, now())",
            &[&rule, &scope],
          )
          .await
          .expect("flag");
      }
      delete_pigeon_rule_rows(&client, "p1").await;
      delete_pigeon_rule_rows(&client, field).await;
      for table in ["rule_state", "rule_alert_state"] {
        let mut sql = String::with_capacity(18 + table.len());
        sql.push_str("SELECT scope FROM ");
        sql.push_str(table);
        let mut scopes: Vec<String> = client
          .query(sql.as_str(), &[])
          .await
          .expect("read")
          .iter()
          .map(|r| r.get(0))
          .collect();
        scopes.sort_unstable();
        assert_eq!(
          scopes,
          ["", field, "p2"],
          "{table}: the flock's and other pigeons' stay"
        );
      }
      client
        .batch_execute("DROP TABLE rule_alert_state, rule_state;")
        .await
        .expect("dropped");
      delete_pigeon_rule_rows(&client, "p2").await;
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_concurrent_merges_keep_both_keys() {
    run(async {
      let db = Scratch::new("merge").await;
      let setup = db.client().await;
      let rule = new_rule(&setup, new_flock(&setup, None).await).await;
      // Round 0 races two inserts of a missing row, round 1 two updates of a present one.
      for round in 0..2 {
        let mut a = db.client().await;
        let mut b = db.client().await;
        let b_pid = pid(&b).await;
        let ta = a.transaction().await.expect("tx");
        let mine = obj(serde_json::json!({ "a": round }));
        assert!(merge_state_row(&ta, rule, "", &mine).await.expect("merge"));
        let second = tokio::spawn(async move {
          let tb = b.transaction().await.expect("tx");
          let theirs = obj(serde_json::json!({ "b": round }));
          let changed = merge_state_row(&tb, rule, "", &theirs)
            .await
            .expect("merge");
          tb.commit().await.expect("commit");
          changed
        });
        wait_until_blocked(b_pid).await;
        ta.commit().await.expect("commit");
        assert!(second.await.expect("second caller"));
        assert_eq!(
          doc(&setup, rule, "").await,
          serde_json::json!({ "a": round, "b": round })
        );
      }

      let mut client = db.client().await;
      let tx = client.transaction().await.expect("tx");
      let same = obj(serde_json::json!({ "b": 1 }));
      assert!(!merge_state_row(&tx, rule, "", &same).await.expect("merge"));
      let absent = obj(serde_json::json!({ "zz": null }));
      assert!(
        !merge_state_row(&tx, rule, "", &absent)
          .await
          .expect("merge")
      );
      assert!(
        !merge_state_row(&tx, rule, "p9", &absent)
          .await
          .expect("merge")
      );
      let gone = obj(serde_json::json!({ "a": null, "n": { "x": null } }));
      assert!(merge_state_row(&tx, rule, "", &gone).await.expect("merge"));
      tx.commit().await.expect("commit");
      assert_eq!(
        doc(&setup, rule, "").await,
        serde_json::json!({ "b": 1, "n": { "x": null } })
      );
      drop((client, setup));
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_the_claim_mails_once_under_two_callers() {
    run(async {
      let db = Scratch::new("claim").await;
      let setup = db.client().await;
      let flock = new_flock(&setup, None).await;
      let rule = new_rule(&setup, flock).await;
      let account = Uuid::from_u128(1);
      let now = DAY + 60;
      for scope in [
        scope_key(RuleAlertScope::Flock, "p1"),
        scope_key(RuleAlertScope::Pigeon, "p1"),
      ] {
        let mut a = db.client().await;
        let mut b = db.client().await;
        let b_pid = pid(&b).await;
        let ta = a.transaction().await.expect("tx");
        let fired = apply_alert(&ta, rule, scope, "down", RuleAlertKind::Fire, now, now)
          .await
          .expect("transition");
        assert!(fired);
        assert!(claim_mail(&ta, rule, account, now).await.expect("claim"));
        let second = tokio::spawn(async move {
          let tb = b.transaction().await.expect("tx");
          let fired = apply_alert(&tb, rule, scope, "down", RuleAlertKind::Fire, now, now)
            .await
            .expect("transition");
          let mailed = fired && claim_mail(&tb, rule, account, now).await.expect("claim");
          tb.commit().await.expect("commit");
          (fired, mailed)
        });
        wait_until_blocked(b_pid).await;
        ta.commit().await.expect("commit");
        assert_eq!(second.await.expect("second caller"), (false, false));
      }
      assert_eq!(mail_sent(&setup, &account.to_string(), now).await, 2);
      assert_eq!(mail_sent(&setup, "platform", now).await, 2);
      let in_hour: i32 = setup
        .query_one("SELECT mail_in_hour FROM rules WHERE id = $1", &[&rule])
        .await
        .expect("rule row")
        .get(0);
      assert_eq!(in_hour, 2);
      drop(setup);
      db.drop().await;
    });
  }

  /// A caller that lost a race: anything but contention fails the test.
  fn lost_race(error: &tokio_postgres::Error) -> bool {
    assert!(
      is_contention(error),
      "an apply failed for a reason other than contention"
    );
    false
  }

  /// Two batches of one rule touching flock keys `a` and `b` in opposite orders, the second
  /// caller blocked on the first's lock before the first takes its second key.
  async fn opposite_orders(db: &Scratch, locked: bool) -> [bool; 2] {
    let setup = db.client().await;
    let rule = new_rule(&setup, new_flock(&setup, None).await).await;
    let account = Uuid::from_u128(2);
    let now = DAY + 60;
    let mut a = db.client().await;
    let mut b = db.client().await;
    let b_pid = pid(&b).await;
    let ta = a.transaction().await.expect("tx");
    let fire = |tx, key| apply_alert(tx, rule, "", key, RuleAlertKind::Fire, now, now);
    if locked {
      assert!(lock_rule(&ta, rule).await.expect("lock"));
    }
    fire(&ta, "a").await.expect("transition");
    let second = tokio::spawn(async move {
      let tb = b.transaction().await.expect("tx");
      let run = async {
        if locked && !lock_rule(&tb, rule).await? {
          return Ok(false);
        }
        let fire = |key| apply_alert(&tb, rule, "", key, RuleAlertKind::Fire, now, now);
        if fire("b").await? | fire("a").await? {
          claim_mail(&tb, rule, account, now).await?;
        }
        Ok::<_, tokio_postgres::Error>(true)
      }
      .await;
      match run {
        Ok(_) => tb.commit().await.is_ok(),
        Err(e) => lost_race(&e),
      }
    });
    wait_until_blocked(b_pid).await;
    let first = match fire(&ta, "b").await {
      Ok(_) => claim_mail(&ta, rule, account, now).await.is_ok() && ta.commit().await.is_ok(),
      Err(e) => lost_race(&e),
    };
    let second = second.await.expect("second caller");
    drop(setup);
    [first, second]
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_batches_of_one_rule_never_deadlock_behind_the_rule_lock() {
    run(async {
      let db = Scratch::new("lockorder").await;
      // Without the rule lock one of the two is aborted as a deadlock, which is what the
      // locked run below could have shown.
      let unlocked = opposite_orders(&db, false).await;
      assert_eq!(unlocked.iter().filter(|ok| **ok).count(), 1);
      assert_eq!(opposite_orders(&db, true).await, [true, true]);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_flag_moves_only_for_a_newer_batch_and_a_flock_flag_holds() {
    run(async {
      let db = Scratch::new("transition").await;
      let mut client = db.client().await;
      let rule = new_rule(&client, new_flock(&client, None).await).await;
      let tx = client.transaction().await.expect("tx");
      let step = |scope: &'static str, key: &'static str, kind, last, now| {
        let tx = &tx;
        async move {
          apply_alert(tx, rule, scope, key, kind, last, now)
            .await
            .expect("transition")
        }
      };
      use RuleAlertKind::{Clear, Fire, Notify};
      let t = DAY;
      // A clear with no flag writes nothing.
      assert!(!step("p1", "never", Clear, t + 100, t + 1_000).await);
      // Pigeon scope: an older or equal batch never undoes a newer change; a restatement is quiet.
      assert!(step("p1", "k", Fire, t + 100, t + 1_000).await);
      assert!(!step("p1", "k", Fire, t + 150, t + 1_000).await);
      assert!(!step("p1", "k", Clear, t + 90, t + 1_000).await);
      assert!(!step("p1", "k", Clear, t + 100, t + 1_000).await);
      assert!(step("p1", "k", Clear, t + 110, t + 1_001).await);
      assert!(!step("p1", "k", Clear, t + 120, t + 1_002).await);
      // Flock scope: at most one flip per FLOCK_FLAG_HOLD_SECS.
      assert!(step("", "k", Fire, t + 100, t + 1_000).await);
      assert!(!step("", "k", Clear, t + 200, t + 1_599).await);
      assert!(step("", "k", Clear, t + 200, t + 1_600).await);
      assert!(!step("", "k", Fire, t + 300, t + 1_700).await);
      assert!(step("", "k", Fire, t + 300, t + 2_200).await);
      // Notify: once per key a week.
      let week = super::super::rules::NOTIFY_REPEAT_SECS;
      assert!(step("p1", "boot", Notify, t + 100, t + 1_000).await);
      assert!(!step("p1", "boot", Notify, t + 100, t + 1_000 + week - 1).await);
      assert!(step("p1", "boot", Notify, t + 100, t + 1_000 + week).await);
      tx.commit().await.expect("commit");
      let flags: i64 = client
        .query_one(
          "SELECT count(*) FROM rule_alert_state WHERE rule_id = $1",
          &[&rule],
        )
        .await
        .expect("flags")
        .get(0);
      assert_eq!(flags, 3);
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_mail_caps_stop_at_their_numbers() {
    use super::super::rules::{
      ACCOUNT_RULE_MAIL_PER_DAY, PLATFORM_RULE_MAIL_PER_DAY, RULE_MAIL_PER_DAY, RULE_MAIL_PER_HOUR,
    };
    run(async {
      let db = Scratch::new("caps").await;
      let mut client = db.client().await;
      let flock = new_flock(&client, None).await;
      let hour = |n: i64| DAY + n * 3_600 + 5;
      let per_hour = RULE_MAIL_PER_HOUR as usize;

      // The rule's hour, then its day, then a new day.
      let account = Uuid::from_u128(3);
      let rule = new_rule(&client, flock).await;
      assert_eq!(
        claims(&mut client, rule, account, hour(0), per_hour + 1).await,
        per_hour
      );
      let mut day = per_hour;
      for n in 1..10 {
        day += claims(&mut client, rule, account, hour(n), per_hour).await;
      }
      assert_eq!(day, RULE_MAIL_PER_DAY as usize);
      assert_eq!(claims(&mut client, rule, account, hour(24), 1).await, 1);

      // The account's day across rules; a refused claim leaves the platform count alone.
      let account = Uuid::from_u128(4);
      let day2 = |n: i64| hour(48 + n);
      let mut granted = 0;
      while granted < ACCOUNT_RULE_MAIL_PER_DAY as usize {
        let rule = new_rule(&client, flock).await;
        for n in 0..5 {
          granted += claims(&mut client, rule, account, day2(n), per_hour).await;
        }
      }
      assert_eq!(granted, ACCOUNT_RULE_MAIL_PER_DAY as usize);
      let rule = new_rule(&client, flock).await;
      assert_eq!(claims(&mut client, rule, account, day2(6), 1).await, 0);
      assert_eq!(
        mail_sent(&client, &account.to_string(), day2(0)).await,
        ACCOUNT_RULE_MAIL_PER_DAY
      );
      assert_eq!(
        mail_sent(&client, "platform", day2(0)).await,
        ACCOUNT_RULE_MAIL_PER_DAY
      );

      // The platform's day, the last slot raced by two callers on other accounts.
      let day3 = hour(96);
      client
        .execute(
          "INSERT INTO rule_usage (scope, day, mail_sent)
           VALUES ('platform', (to_timestamp($1::bigint) AT TIME ZONE 'UTC')::date, $2)",
          &[&day3, &(PLATFORM_RULE_MAIL_PER_DAY - 1)],
        )
        .await
        .expect("platform row seeded");
      let (rule_a, rule_b) = (
        new_rule(&client, flock).await,
        new_rule(&client, flock).await,
      );
      let mut a = db.client().await;
      let mut b = db.client().await;
      let b_pid = pid(&b).await;
      let ta = a.transaction().await.expect("tx");
      assert!(
        claim_mail(&ta, rule_a, Uuid::from_u128(5), day3)
          .await
          .expect("claim")
      );
      let second = tokio::spawn(async move {
        let tb = b.transaction().await.expect("tx");
        let claimed = claim_mail(&tb, rule_b, Uuid::from_u128(6), day3)
          .await
          .expect("claim");
        tb.commit().await.expect("commit");
        claimed
      });
      wait_until_blocked(b_pid).await;
      ta.commit().await.expect("commit");
      assert!(!second.await.expect("second caller"));
      assert_eq!(
        mail_sent(&client, "platform", day3).await,
        PLATFORM_RULE_MAIL_PER_DAY
      );
      drop(client);
      db.drop().await;
    });
  }
  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_two_concurrent_deploys_leave_one_holding_the_lease() {
    run(async {
      let db = Scratch::new("lease").await;
      let a = db.client().await;
      let b = db.client().await;
      let b_pid = pid(&b).await;

      // Forced: the first deploy's insert is uncommitted when the second arrives.
      let flock = new_flock(&a, None).await;
      a.batch_execute("BEGIN").await.expect("begin");
      let held = take_deploy_lease(&a, flock, 1, "acct-x")
        .await
        .expect("lease")
        .expect("the first deploy holds it");
      let second = tokio::spawn(async move {
        let got = take_deploy_lease(&b, flock, 1, "acct-x")
          .await
          .expect("lease");
        (b, got)
      });
      wait_until_blocked(b_pid).await;
      a.batch_execute("COMMIT").await.expect("commit");
      let (b, got) = second.await.expect("second caller");
      assert_eq!(got, None);

      // Unforced: both race on fresh flocks, existing rows and missing ones alike.
      for round in 0..20 {
        let flock = new_flock(&a, None).await;
        if round % 2 == 1 {
          let lease = take_deploy_lease(&a, flock, 1, "acct-x")
            .await
            .expect("lease");
          release_deploy_lease(&a, flock, lease.expect("free"))
            .await
            .expect("release");
        }
        let (one, two) = futures::join!(
          take_deploy_lease(&a, flock, 1, "acct-x"),
          take_deploy_lease(&b, flock, 1, "acct-x"),
        );
        let holders = [one.expect("lease"), two.expect("lease")];
        assert_eq!(holders.iter().flatten().count(), 1);
      }

      // A lease that outlived its deploy is taken over, and the late holder cannot free it.
      a.execute(
        "UPDATE rule_bundles SET deploy_until = now() - interval '1 second' WHERE flock_id = $1",
        &[&flock],
      )
      .await
      .expect("lease expired");
      let taken = take_deploy_lease(&b, flock, 1, "acct-x")
        .await
        .expect("lease")
        .expect("an expired lease is free");
      assert!(
        !release_deploy_lease(&a, flock, held)
          .await
          .expect("release")
      );
      assert_eq!(
        take_deploy_lease(&a, flock, 1, "acct-x")
          .await
          .expect("lease"),
        None
      );
      assert!(
        release_deploy_lease(&b, flock, taken)
          .await
          .expect("release")
      );
      assert!(
        take_deploy_lease(&a, flock, 1, "acct-x")
          .await
          .expect("lease")
          .is_some()
      );
      drop((a, b));
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_assistant_turns_land_on_the_account_and_the_platform() {
    run(async {
      let db = Scratch::new("usage").await;
      let a = db.client().await;
      let b = db.client().await;
      let turn = AssistantUsage {
        input_tokens: 3_000,
        output_tokens: 1_200,
        cache_read_tokens: 9_000,
        cache_write_tokens: 4_000,
        cost_micro_usd: 130_000,
      };
      let now = DAY + 30;
      let (acct_a, acct_b) = (Uuid::from_u128(7), Uuid::from_u128(8));
      let (one, two) = futures::join!(
        record_assistant_turn(&a, acct_a, now, &turn),
        record_assistant_turn(&b, acct_a, now, &turn),
      );
      one.expect("turn recorded");
      two.expect("turn recorded");
      record_assistant_turn(&a, acct_b, now, &turn)
        .await
        .expect("turn recorded");
      record_assistant_turn(&a, acct_a, now + 86_400, &turn)
        .await
        .expect("turn recorded");
      let (acct_a, acct_b) = (acct_a.to_string(), acct_b.to_string());
      let totals = |scope: &str, at: i64| {
        let (a, scope) = (&a, scope.to_string());
        async move {
          let row = a
            .query_one(
              "SELECT assistant_turns, input_tokens, output_tokens, cache_read_tokens,
                 cache_write_tokens, cost_micro_usd FROM rule_usage
               WHERE scope = $1 AND day = (to_timestamp($2::bigint) AT TIME ZONE 'UTC')::date",
              &[&scope, &at],
            )
            .await
            .expect("usage row");
          let turns: i32 = row.get(0);
          let sums: [i64; 5] = std::array::from_fn(|i| row.get(i + 1));
          (turns, sums)
        }
      };
      let times = |n: i64| [3_000 * n, 1_200 * n, 9_000 * n, 4_000 * n, 130_000 * n];
      assert_eq!(totals(&acct_a, now).await, (2, times(2)));
      assert_eq!(totals(&acct_b, now).await, (1, times(1)));
      assert_eq!(totals("platform", now).await, (3, times(3)));
      assert_eq!(totals(&acct_a, now + 86_400).await, (1, times(1)));
      assert_eq!(totals("platform", now + 86_400).await, (1, times(1)));
      drop((a, b));
      db.drop().await;
    });
  }
  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_the_rule_cap_stops_at_the_served_tier() {
    use crate::helpers::EntitlementCap;
    use crate::helpers::firmware::FlockAccess;
    use crate::helpers::usage::check_rule_cap;
    run(async {
      let db = Scratch::new("rulecap").await;
      let client = db.client().await;
      let refused = |flock: Uuid| {
        let client = &client;
        async move {
          let access = FlockAccess::assert_checked(&flock.to_string(), None);
          matches!(
            check_rule_cap(client, &access).await,
            EntitlementCap::Refuse(_)
          )
        }
      };
      let org = |plan: &'static str, status: &'static str| {
        let client = &client;
        async move {
          let row = client
            .query_one(
              "INSERT INTO organizations (name, plan, subscription_status) VALUES ('o', $1, $2)
               RETURNING id",
              &[&plan, &status],
            )
            .await
            .expect("org inserted");
          row.get::<_, Uuid>(0)
        }
      };

      // The free tier: one rule per account, whichever flock holds it; another account's rules
      // never count.
      let mine = new_flock(&client, None).await;
      let theirs = new_flock(&client, None).await;
      new_rule(&client, theirs).await;
      assert!(!refused(mine).await);
      new_rule(&client, mine).await;
      assert!(refused(mine).await);

      // Builder: ten across the organization's flocks.
      let builder = org("builder", "active").await;
      let (one, two) = (
        new_flock(&client, Some(builder)).await,
        new_flock(&client, Some(builder)).await,
      );
      for n in 0..9 {
        new_rule(&client, if n % 2 == 0 { one } else { two }).await;
      }
      assert!(!refused(two).await);
      new_rule(&client, one).await;
      assert!(refused(two).await);

      // A lapsed subscription is served as the free tier; Growth is unlimited.
      let lapsed = org("builder", "canceled").await;
      let flock = new_flock(&client, Some(lapsed)).await;
      new_rule(&client, flock).await;
      assert!(refused(flock).await);
      let growth = new_flock(&client, Some(org("growth", "active").await)).await;
      for _ in 0..25 {
        new_rule(&client, growth).await;
      }
      assert!(!refused(growth).await);
      drop(client);
      db.drop().await;
    });
  }
  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_the_history_read_returns_no_row_under_a_field_pigeon() {
    use crate::helpers::firmware::FlockAccess;
    use crate::helpers::query_telemetry_history_for_flock;
    use crate::helpers::rules::{FIELD_PIGEONS, excluded_pigeons};
    run(async {
      let db = Scratch::new("history").await;
      let client = db.client().await;
      let flock = new_flock(&client, None).await;
      let other = "0".repeat(64);
      for id in FIELD_PIGEONS.iter().copied().chain([other.as_str()]) {
        new_pigeon(&client, flock, id).await;
        client
          .execute(
            "INSERT INTO pigeon_telemetry_history (pigeon_id, key, value, value_num, reported_at)
             VALUES ($1, 'rsrp_dbm', '-97', -97, now() - interval '1 hour')",
            &[&id],
          )
          .await
          .expect("history row");
      }
      let access = FlockAccess::assert_checked(&flock.to_string(), None);
      let read = |exclude: Vec<String>| {
        let (client, access) = (&client, &access);
        async move {
          let page = query_telemetry_history_for_flock(client, access, None, None, None, &exclude)
            .await
            .expect("history read");
          let mut ids: Vec<String> = page.points.into_iter().map(|p| p.pigeon_id).collect();
          ids.sort();
          ids
        }
      };
      // Without the argument every pigeon's row is there to be read.
      let mut all: Vec<String> = FIELD_PIGEONS.iter().map(|id| id.to_string()).collect();
      all.push(other.clone());
      all.sort();
      assert_eq!(read(Vec::new()).await, all);
      assert_eq!(read(excluded_pigeons(None)).await, vec![other.clone()]);
      assert_eq!(read(excluded_pigeons(Some(""))).await, vec![other.clone()]);
      assert_eq!(
        read(excluded_pigeons(Some(&other))).await,
        Vec::<String>::new()
      );
      // The rule-side read builds the list itself.
      let since = OffsetDateTime::now_utc() - time::Duration::days(7);
      let replay = |var: Option<&'static str>| {
        let (client, access) = (&client, &access);
        async move {
          let page = replay_history(client, access, since, var)
            .await
            .expect("replay read");
          page
            .points
            .into_iter()
            .map(|p| p.pigeon_id)
            .collect::<Vec<_>>()
        }
      };
      assert_eq!(replay(None).await, vec![other.clone()]);
      assert_eq!(replay(Some("")).await, vec![other.clone()]);
      drop(client);
      db.drop().await;
    });
  }

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

  /// Every rule in `rules` enabled at `version`.
  async fn deploy_enabled(client: &Client, rules: &[Uuid], version: i32) {
    client
      .execute(
        "UPDATE rules SET enabled = true, deployed_version = $2 WHERE id = ANY($1)",
        &[&rules, &version],
      )
      .await
      .expect("deployed");
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_the_pass_reads_only_live_rules_and_this_pigeons_state() {
    run(async {
      let db = Scratch::new("pass_read").await;
      let client = db.client().await;
      let org = new_org(&client).await;
      let flock = new_flock(&client, Some(org)).await;
      new_pigeon(&client, flock, "p1").await;
      new_pigeon(&client, flock, "p2").await;
      assert!(!rules_gate(&client, "p1").await.expect("gate"), "no rules");
      let a = new_rule(&client, flock).await;
      let b = new_rule(&client, flock).await;
      let off = new_rule(&client, flock).await;
      let blocked = new_rule(&client, flock).await;
      let undeployed = new_rule(&client, flock).await;
      deploy_enabled(&client, &[a, b, blocked], 2).await;
      client
        .batch_execute(&format!(
          "UPDATE rules SET blocked_at = now() WHERE id = '{blocked}';
           UPDATE rules SET enabled = true WHERE id = '{undeployed}';
           UPDATE pigeons SET name = 'Stop 9' WHERE id = 'p1';"
        ))
        .await
        .expect("fixtures");
      assert!(rules_gate(&client, "p1").await.expect("gate"));

      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("pigeon found");
      assert_eq!(found.uploaded_at, None, "no bundle row yet");
      assert!(!found.leased);
      assert_eq!((found.account, found.org), (org, Some(org)));
      assert_eq!(
        (found.flock.id.clone(), found.flock.name.as_str()),
        (flock.to_string(), "f")
      );
      assert_eq!(
        (found.pigeon.id.as_str(), found.pigeon.name.as_str()),
        ("p1", "Stop 9")
      );
      let ids: Vec<Uuid> = found.rules.iter().map(|r| r.id).collect();
      assert_eq!(ids, [a, b], "{off} {blocked} {undeployed} are not live");
      assert!(found.rules.iter().all(|r| r.version == 2 && !r.failing));

      client
        .batch_execute(&format!(
          "INSERT INTO rule_state (rule_id, scope, doc) VALUES
             ('{a}', 'p1', '{{\"x\": 1}}'), ('{a}', 'p2', '{{\"y\": 2}}'),
             ('{a}', '', '{{\"z\": 3}}');
           INSERT INTO rule_alert_state (rule_id, scope, key, firing, decided_at, changed_at,
             last_sent_at) VALUES
             ('{a}', 'p1', 'k', true, to_timestamp(100), to_timestamp(200), NULL),
             ('{a}', 'p2', 'k2', true, to_timestamp(100), to_timestamp(200), NULL),
             ('{a}', '', 'kf', false, to_timestamp(100), to_timestamp(200), to_timestamp(300));
           UPDATE rules SET mail_hour = to_timestamp(7200), mail_in_hour = 4,
             mail_day = DATE '1970-01-02', mail_in_day = 9, fail_count = 2 WHERE id = '{a}';
           UPDATE rules SET watch_until = now() + interval '1 hour', pending_version = 3
             WHERE id = '{b}';
           INSERT INTO rule_usage (scope, day, mail_sent) VALUES
             ('{org}', (now() AT TIME ZONE 'UTC')::date, 5),
             ('platform', (now() AT TIME ZONE 'UTC')::date, 6),
             ('{org}', DATE '2000-01-01', 50);
           INSERT INTO rule_bundles (flock_id, contract, account_tag, uploaded_at, deploy_until)
             VALUES ('{flock}', 1, 't', now(), now() + interval '1 minute');"
        ))
        .await
        .expect("state");
      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("pigeon found");
      assert!(
        found
          .uploaded_at
          .is_some_and(|at| (found.now - at).abs() < 5)
      );
      assert!(found.leased);
      assert_eq!((found.account_mail_day, found.platform_mail_day), (5, 6));
      let (ra, rb) = (&found.rules[0], &found.rules[1]);
      assert_eq!(
        Value::Object(ra.state.pigeon.clone()),
        serde_json::json!({ "x": 1 })
      );
      assert_eq!(
        Value::Object(ra.state.flock.clone()),
        serde_json::json!({ "z": 3 })
      );
      assert_eq!(
        ra.flags.len(),
        2,
        "another pigeon's flag is not this batch's"
      );
      let flag = ra.flags[&(RuleAlertScope::Pigeon, "k".to_string())];
      assert_eq!(
        (flag.firing, flag.decided_at, flag.changed_at),
        (true, 100, 200)
      );
      assert_eq!(flag.last_sent_at, None);
      let flock_flag = ra.flags[&(RuleAlertScope::Flock, "kf".to_string())];
      assert_eq!(flock_flag.last_sent_at, Some(300));
      assert_eq!(
        ra.mail,
        RuleMailCounters {
          hour_start: Some(7200),
          in_hour: 4,
          day: Some(1),
          in_day: 9,
        }
      );
      assert!(ra.failing && !ra.watched && !ra.pending);
      assert!(rb.watched && rb.pending && rb.state == RuleState::default());

      client
        .batch_execute("UPDATE pigeons SET suspended_at = now() WHERE id = 'p1'")
        .await
        .expect("suspend");
      assert!(!rules_gate(&client, "p1").await.expect("gate"));
      assert!(fresh_read(&client, "p1").await.expect("read").is_none());
      assert!(fresh_read(&client, "nope").await.expect("read").is_none());
      let bare = fresh_read(&client, "p2")
        .await
        .expect("read")
        .expect("found");
      assert_eq!(bare.pigeon.name, "p2", "an unnamed pigeon goes by its id");
      drop(client);
      db.drop().await;
    });
  }

  fn one_reading(at: i64) -> RuleWork {
    let metrics = [("k".to_string(), "1".to_string())].into_iter().collect();
    let payload = at.to_string();
    RuleWork::new(
      "p1",
      &payload,
      false,
      vec![ResolvedReading::new(at, metrics)],
    )
    .expect("one reading")
  }

  /// One rule's answer as the harness gives it.
  fn answered(rule: Uuid, status: &str, out_json: &str) -> Classified {
    answered_all(&[(rule, status, out_json)])
  }

  /// The harness's answer for `runs` of (rule, status, out_json), each at version 1.
  fn answered_all(runs: &[(Uuid, &str, &str)]) -> Classified {
    let results: Vec<Value> = runs
      .iter()
      .map(|(rule, status, out_json)| {
        serde_json::json!({
          "id": uuid_str(*rule), "status": status, "out_json": out_json, "error": "e", "logs": []
        })
      })
      .collect();
    let body = serde_json::json!({ "contract": 1, "results": results }).to_string();
    let sent: Vec<DispatchRule> = runs
      .iter()
      .map(|(rule, _, _)| DispatchRule {
        id: uuid_str(*rule),
        version: 1,
        state: RuleState::default(),
      })
      .collect();
    classify(
      &sent,
      DispatchOutcome::Answered {
        status: 200,
        body: body.into_bytes(),
      },
    )
  }

  async fn rule_row(client: &Client, rule: Uuid) -> (bool, i32, Option<String>) {
    let row = client
      .query_one(
        "SELECT enabled, fail_count, paused_reason FROM rules WHERE id = $1",
        &[&rule],
      )
      .await
      .expect("rule row");
    (row.get(0), row.get(1), row.get(2))
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_mail_targets_carry_the_list_and_the_accounts_addresses() {
    run(async {
      let db = Scratch::new("pass_mail").await;
      let client = db.client().await;
      let org = new_org(&client).await;
      let flock = new_flock(&client, Some(org)).await;
      let rule = new_rule(&client, flock).await;
      let other = new_rule(&client, new_flock(&client, None).await).await;
      client
        .execute(
          "UPDATE rules SET name = 'Dark', recipients = '[\"ops@x.io\"]' WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("list");
      client
        .execute(
          "UPDATE flocks SET owner_email = 'own@x.io' WHERE id = $1",
          &[&flock],
        )
        .await
        .expect("owner");
      for email in ["ops@x.io", "b@x.io"] {
        client
          .execute(
            "INSERT INTO organization_members (org_id, user_id, role, email)
             VALUES ($1, gen_random_uuid(), 'member', $2)",
            &[&org, &email],
          )
          .await
          .expect("member");
      }
      let targets = mail_targets(&client, &[rule]).await.expect("read");
      assert_eq!(targets.len(), 1, "only the rules asked for");
      let target = &targets[0];
      assert_eq!((target.rule_id, target.name.as_str()), (rule, "Dark"));
      assert_eq!(target.recipients, ["ops@x.io"]);
      assert_eq!(target.owner_email.as_deref(), Some("own@x.io"));
      let mut members: Vec<&str> = target
        .member_emails
        .as_deref()
        .unwrap_or("")
        .split(',')
        .collect();
      members.sort_unstable();
      assert_eq!(members, ["b@x.io", "ops@x.io"]);
      assert_eq!(
        target.author_email, None,
        "an author outside the org is mailed nothing"
      );
      client
        .execute(
          "INSERT INTO organization_members (org_id, user_id, role, email)
           SELECT $1, created_by, 'admin', 'author@x.io' FROM rules WHERE id = $2",
          &[&org, &rule],
        )
        .await
        .expect("author joins");
      let targets = mail_targets(&client, &[rule]).await.expect("read");
      assert_eq!(targets[0].author_email.as_deref(), Some("author@x.io"));
      let lone = mail_targets(&client, &[other]).await.expect("read");
      assert_eq!(
        (lone[0].owner_email.clone(), lone[0].member_emails.clone()),
        (None, None)
      );
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_run_records_its_plan_and_its_streak() {
    run(async {
      let db = Scratch::new("pass_record").await;
      let mut client = db.client().await;
      let org = new_org(&client).await;
      let flock = new_flock(&client, Some(org)).await;
      new_pigeon(&client, flock, "p1").await;
      let rule = new_rule(&client, flock).await;
      deploy_enabled(&client, &[rule], 1).await;
      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("found");
      let at = found.now - 5;
      let work = one_reading(at);
      let out = r#"{"state":{"pigeon":{"n":1},"flock":{"m":2}},"values":[{"key":"v","value":"7"}],
        "alerts":[{"key":"down","kind":"fire","severity":"critical","scope":"pigeon",
        "subject":"s"}]}"#;
      let plans = plan_record(&found, &work.readings, &answered(rule, "ok", out));
      let mails = recorded(record_runs(&mut client, &found, &work, &plans, 42).await);
      assert_eq!(mails.len(), 1);
      let RuleMailKind::Alert(alert) = &mails[0].kind else {
        panic!("an alert mail");
      };
      assert_eq!((mails[0].rule_id, alert.key.as_str()), (rule, "down"));
      assert_eq!(
        doc(&client, rule, "p1").await,
        serde_json::json!({ "n": 1 })
      );
      assert_eq!(doc(&client, rule, "").await, serde_json::json!({ "m": 2 }));
      let row = client
        .query_one(
          "SELECT status, readings, alerts_sent, alerts_suppressed, values_written,
             state_changed, wall_ms, version FROM rule_runs WHERE rule_id = $1",
          &[&rule],
        )
        .await
        .expect("one run row");
      let status: String = row.get(0);
      let counts: (i32, i32, i32, i32) = (row.get(1), row.get(2), row.get(3), row.get(4));
      assert_eq!((status.as_str(), counts), ("ok", (1, 1, 0, 1)));
      let tail: (bool, Option<i32>, i32) = (row.get(5), row.get(6), row.get(7));
      assert_eq!(tail, (true, Some(42), 1));
      let value: String = client
        .query_one("SELECT value FROM rule_values WHERE rule_id = $1", &[&rule])
        .await
        .expect("value row")
        .get(0);
      assert_eq!(value, "7");
      assert_eq!(mail_sent(&client, &org.to_string(), found.now).await, 1);

      // A redelivery keeps the first row and mails nothing more.
      let again = recorded(record_runs(&mut client, &found, &work, &plans, 42).await);
      assert!(again.is_empty());
      let runs: i64 = client
        .query_one("SELECT count(*) FROM rule_runs", &[])
        .await
        .expect("count")
        .get(0);
      assert_eq!(runs, 1);

      // Twenty throws in a row pause the rule, each read under the row's lock, and the run that
      // paused it owes its author one mail.
      let mut paused = Vec::new();
      for i in 1..=AUTO_PAUSE_RUNS {
        let work = one_reading(at + i64::from(i));
        let plans = plan_record(&found, &work.readings, &answered(rule, "threw", ""));
        paused.extend(recorded(
          record_runs(&mut client, &found, &work, &plans, 1).await,
        ));
      }
      assert_eq!(
        rule_row(&client, rule).await,
        (false, AUTO_PAUSE_RUNS, Some("auto:threw".to_string()))
      );
      assert_eq!(paused.len(), 1);
      assert!(matches!(
        &paused[0].kind,
        RuleMailKind::Paused { reason, error }
          if reason == "auto:threw" && error.as_deref() == Some("e")
      ));

      // A quiet success after a failure writes the reset alone.
      client
        .execute(
          "UPDATE rules SET enabled = true, paused_reason = NULL, fail_count = 3,
             fail_since = now() WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("re-enabled");
      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("found");
      let work = one_reading(at + 100);
      let plans = plan_record(&found, &work.readings, &answered(rule, "ok", "{}"));
      assert!(plans[0].writes && !plans[0].run_row);
      recorded(record_runs(&mut client, &found, &work, &plans, 1).await);
      assert_eq!(rule_row(&client, rule).await, (true, 0, None));

      // A whole failure is every rule's, and a rule deleted since the read writes nothing.
      let flock_failed = Classified::Flock(RunStatus::DispatchFailed);
      let work = one_reading(at + 200);
      let plans = plan_record(&found, &work.readings, &flock_failed);
      recorded(record_runs(&mut client, &found, &work, &plans, 2_000).await);
      assert_eq!(rule_row(&client, rule).await, (true, 1, None));
      client
        .execute("DELETE FROM rules WHERE id = $1", &[&rule])
        .await
        .expect("deleted");
      let work = one_reading(at + 300);
      assert!(recorded(record_runs(&mut client, &found, &work, &plans, 2_000).await).is_empty());
      assert!(
        mail_targets(&client, &[rule])
          .await
          .expect("read")
          .is_empty()
      );
      drop(client);
      db.drop().await;
    });
  }

  /// The mails of a record that failed nowhere.
  fn recorded((mails, failed): (Vec<RuleMail>, Option<tokio_postgres::Error>)) -> Vec<RuleMail> {
    assert!(failed.is_none(), "recorded");
    mails
  }

  /// Rows any run could have written, across the four tables a run writes.
  async fn run_writes(client: &Client) -> i64 {
    client
      .query_one(
        "SELECT (SELECT count(*) FROM rule_state) + (SELECT count(*) FROM rule_runs)
           + (SELECT count(*) FROM rule_alert_state) + (SELECT count(*) FROM rule_values)",
        &[],
      )
      .await
      .expect("count")
      .get(0)
  }

  /// Records `plans` as batch `at` and returns how many mails it claimed.
  async fn mailed(client: &mut Client, found: &Lookup, at: i64, plans: &[RulePlan]) -> usize {
    let (mails, failed) = record_runs(client, found, &one_reading(at), plans, 1).await;
    assert!(failed.is_none());
    mails.len()
  }

  const FIRE_DOWN: &str = r#"{"state":{"pigeon":{"n":1}},"alerts":[{"key":"down","kind":"fire",
    "severity":"critical","scope":"pigeon","subject":"s"}]}"#;

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_run_read_before_a_promote_disable_or_block_writes_nothing() {
    run(async {
      let db = Scratch::new("pass_stale").await;
      let mut client = db.client().await;
      let org = new_org(&client).await;
      let flock = new_flock(&client, Some(org)).await;
      new_pigeon(&client, flock, "p1").await;
      let rule = new_rule(&client, flock).await;
      deploy_enabled(&client, &[rule], 1).await;
      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("found");
      let at = found.now - 5;
      let ok = plan_record(&found, &[], &answered(rule, "ok", FIRE_DOWN));
      let threw = plan_record(&found, &[], &answered(rule, "threw", ""));

      // Promoted to version 2 with its state reset since the read.
      deploy_enabled(&client, &[rule], 2).await;
      assert_eq!(mailed(&mut client, &found, at + 1, &ok).await, 0);
      assert_eq!(mailed(&mut client, &found, at + 2, &threw).await, 0);
      assert_eq!(run_writes(&client).await, 0, "no v1 state lands in v2");
      assert_eq!(rule_row(&client, rule).await, (true, 0, None));

      // Disabled by its owner.
      deploy_enabled(&client, &[rule], 1).await;
      client
        .execute(
          "UPDATE rules SET enabled = false, paused_reason = 'owner' WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("disabled");
      assert_eq!(mailed(&mut client, &found, at + 3, &ok).await, 0);
      assert_eq!(mailed(&mut client, &found, at + 4, &threw).await, 0);
      assert_eq!(
        rule_row(&client, rule).await,
        (false, 0, Some("owner".to_string()))
      );

      // Blocked by the platform.
      client
        .execute(
          "UPDATE rules SET enabled = true, paused_reason = NULL, blocked_at = now()
           WHERE id = $1",
          &[&rule],
        )
        .await
        .expect("blocked");
      assert_eq!(mailed(&mut client, &found, at + 5, &ok).await, 0);
      assert_eq!(mailed(&mut client, &found, at + 6, &threw).await, 0);
      assert_eq!(run_writes(&client).await, 0);

      // The same plans write once the rule is what the read saw.
      client
        .execute("UPDATE rules SET blocked_at = NULL WHERE id = $1", &[&rule])
        .await
        .expect("unblocked");
      assert_eq!(mailed(&mut client, &found, at + 7, &ok).await, 1);
      assert!(run_writes(&client).await > 0);
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_failing_rule_keeps_its_siblings_mail_and_a_refused_result_is_bad_output() {
    run(async {
      let db = Scratch::new("pass_partial").await;
      let mut client = db.client().await;
      let org = new_org(&client).await;
      let flock = new_flock(&client, Some(org)).await;
      new_pigeon(&client, flock, "p1").await;
      let a = new_rule(&client, flock).await;
      let b = new_rule(&client, flock).await;
      deploy_enabled(&client, &[a, b], 1).await;
      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("found");
      let at = found.now - 5;
      let values = r#"{"values":[{"key":"v","value":"7"}]}"#;

      // Postgres refuses b's value: b records bad_output and nothing else, a is untouched.
      let work = one_reading(at);
      let mut plans = plan_record(
        &found,
        &work.readings,
        &answered_all(&[(a, "ok", FIRE_DOWN), (b, "ok", values)]),
      );
      plans[1].apply.as_mut().expect("applies").values[0].value = "a\0b".to_string();
      let (mails, failed) = record_runs(&mut client, &found, &work, &plans, 1).await;
      assert!(
        failed.is_none(),
        "one rule's refused result does not end the pass"
      );
      assert_eq!(mails.len(), 1);
      let status: String = client
        .query_one("SELECT status FROM rule_runs WHERE rule_id = $1", &[&b])
        .await
        .expect("b's run row")
        .get(0);
      assert_eq!(status, "bad_output");
      assert_eq!(rule_row(&client, b).await, (true, 1, None));
      let values: i64 = client
        .query_one("SELECT count(*) FROM rule_values", &[])
        .await
        .expect("count")
        .get(0);
      assert_eq!(values, 0);

      // b's record fails outright: a's committed mail still comes back with the error.
      let found = fresh_read(&client, "p1")
        .await
        .expect("read")
        .expect("found");
      let fire_up = FIRE_DOWN.replace("down", "up");
      let work = one_reading(at + 1);
      let plans = plan_record(
        &found,
        &work.readings,
        &answered_all(&[(a, "ok", &fire_up), (b, "threw", "")]),
      );
      // A lock held elsewhere answers an error under the transaction's own lock_timeout.
      let mut other = db.client().await;
      let hold = other.transaction().await.expect("tx");
      hold
        .execute("SELECT 1 FROM rules WHERE id = $1 FOR UPDATE", &[&b])
        .await
        .expect("b locked");
      let (mails, failed) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        record_runs(&mut client, &found, &work, &plans, 1),
      )
      .await
      .expect("a lock wait ends in an error, not a hang");
      assert_eq!(mails.len(), 1, "a committed before b failed");
      assert!(matches!(&mails[0].kind, RuleMailKind::Alert(a) if a.key == "up"));
      assert_eq!(
        failed.and_then(|e| e.code().cloned()),
        Some(SqlState::LOCK_NOT_AVAILABLE)
      );
      hold.rollback().await.expect("released");
      drop(other);
      drop(client);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_run_that_writes_nothing_pipelines_nothing_behind_it() {
    run(async {
      let db = Scratch::new("pass_piped").await;
      let setup = db.client().await;
      let org = new_org(&setup).await;
      let flock = new_flock(&setup, Some(org)).await;
      new_pigeon(&setup, flock, "p1").await;
      let a = new_rule(&setup, flock).await;
      let b = new_rule(&setup, flock).await;
      deploy_enabled(&setup, &[a, b], 1).await;
      let found = fresh_read(&setup, "p1")
        .await
        .expect("read")
        .expect("found");
      let work = one_reading(found.now - 5);
      let plans = plan_record(
        &found,
        &work.readings,
        &answered_all(&[(a, "ok", FIRE_DOWN), (b, "ok", FIRE_DOWN)]),
      );
      setup
        .execute("UPDATE rules SET enabled = false WHERE id = $1", &[&a])
        .await
        .expect("a disabled since the read");
      let (mut client, piped) = pipeline_watched(&db).await;
      let (mails, failed) = record_runs(&mut client, &found, &work, &plans, 1).await;
      assert!(failed.is_none());
      assert_eq!(
        mails.len(),
        1,
        "b still records behind a's empty transaction"
      );
      assert_eq!(
        piped.load(Ordering::SeqCst),
        0,
        "nothing sent behind a ROLLBACK"
      );
      drop(client);
      drop(setup);
      db.drop().await;
    });
  }

  #[test]
  #[ignore = "needs DOVECOTE_TEST_PG: the dev docker Postgres"]
  fn rule_store_a_rule_transaction_bounds_its_waits_and_only_its_own() {
    run(async {
      let db = Scratch::new("tx_limits").await;
      let mut client = db.client().await;
      async fn limits(client: &impl tokio_postgres::GenericClient) -> (String, String) {
        let row = client
          .query_one(
            "SELECT current_setting('lock_timeout'), current_setting('statement_timeout')",
            &[],
          )
          .await
          .expect("settings");
        (row.get(0), row.get(1))
      }
      let tx = begin(&mut client).await.expect("begin");
      assert_eq!(limits(&tx).await, ("2s".into(), "10s".into()));
      end(tx, Ok::<_, tokio_postgres::Error>(()), |e| e)
        .await
        .expect("commit");
      assert_eq!(
        limits(&client).await,
        ("0".into(), "0".into()),
        "SET LOCAL leaves a pooled connection as it found it"
      );
      drop(client);
      db.drop().await;
    });
  }
}
