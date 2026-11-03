//! The Postgres side of user-authored rules: the tables, and the statements whose concurrency the
//! apply depends on. The pure plan in [`super::rules`] decides on a snapshot; each statement here
//! re-checks on the row it finds, so racing batches and deploys agree with the plan or do nothing.
#![cfg_attr(
  not(all(test, not(target_arch = "wasm32"))),
  expect(
    dead_code,
    reason = "its callers are the queue's second pass and the rule routes"
  )
)]

use std::sync::atomic::{AtomicBool, Ordering};

use crate::helpers::missing_schema_objects;
use crate::helpers::rules::{
  ACCOUNT_RULE_MAIL_PER_DAY, FLOCK_FLAG_HOLD_SECS, NOTIFY_REPEAT_SECS, PLATFORM_RULE_MAIL_PER_DAY,
  RULE_MAIL_PER_DAY, RULE_MAIL_PER_HOUR,
};
use capsules::{RuleAlertKind, RuleAlertScope};
use serde_json::{Map, Value};
use tokio_postgres::{Client, Transaction, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Every table and index [`RULE_TABLES_DDL`] creates, as `missing_schema_objects` names them.
const RULE_RELATIONS: [&str; 10] = [
  "rules",
  "idx_rules_flock_id",
  "rule_versions",
  "rule_state",
  "rule_alert_state",
  "rule_values",
  "rule_runs",
  "idx_rule_runs_rule_ran",
  "rule_bundles",
  "rule_usage",
];

/// The rule tables, verbatim in `infra/migrations/2026-11-03-rules.sql` and `infra/init-db.sql`
/// (a test holds all three together). Idempotent, and creates no trigger: `CREATE TRIGGER` has
/// no `IF NOT EXISTS` here, so every write sets `updated_at` itself.
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
-- undoes a newer one.
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

/// Set once this isolate has found every rule table and index.
static TABLES_READY: AtomicBool = AtomicBool::new(false);

/// Ensures, once per isolate, the rule tables and their indexes, logging anything missing first:
/// the migration should already have created them.
pub async fn ensure_rule_tables(client: &Client) -> Result<()> {
  if TABLES_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  let missing = missing_schema_objects(client, &RULE_RELATIONS, &[]).await?;
  if missing.is_empty() {
    TABLES_READY.store(true, Ordering::Relaxed);
    return Ok(());
  }
  console_error!(
    "ensure_rule_tables: creating missing {}",
    missing.join(", ")
  );
  client.batch_execute(RULE_TABLES_DDL).await.map_err(|e| {
    console_error!("ensure_rule_tables failed: {e}");
    Error::RustError("Internal Server Error".into())
  })?;
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
  SELECT $1, $2, p.patch - p.gone FROM p
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
    SELECT $2, t.day, 1 FROM t WHERE EXISTS (SELECT 1 FROM r)
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

/// Merges `patch` into the rule's document for `scope` ([`scope_key`]). True when it wrote.
pub async fn merge_state(
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
/// is the flock owner's organization or user id, as text. False is a suppressed mail.
pub async fn claim_mail(
  tx: &Transaction<'_>,
  rule_id: Uuid,
  account: &str,
  now: i64,
) -> std::result::Result<bool, tokio_postgres::Error> {
  let row = tx
    .query_typed_one(
      CLAIM_SQL,
      &[
        (&rule_id, Type::UUID),
        (&account, Type::TEXT),
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

#[cfg(test)]
mod tests {
  use super::*;

  const MIGRATION: &str = include_str!("../../../infra/migrations/2026-11-03-rules.sql");
  const INIT_DB: &str = include_str!("../../../infra/init-db.sql");

  #[test]
  fn the_migration_and_init_db_carry_the_ensured_tables_verbatim() {
    assert!(MIGRATION.contains(RULE_TABLES_DDL));
    assert!(INIT_DB.contains(RULE_TABLES_DDL));
  }

  #[test]
  fn every_ensured_relation_is_created_by_the_ddl() {
    for name in RULE_RELATIONS {
      let mut table = String::with_capacity(27 + name.len());
      table.push_str("CREATE TABLE IF NOT EXISTS ");
      table.push_str(name);
      table.push_str(" (");
      let mut index = String::with_capacity(27 + name.len());
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
  use std::future::Future;
  use tokio::net::TcpStream;
  use tokio_postgres::config::Host;
  use tokio_postgres::{Config, NoTls};

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
  async fn claims(client: &mut Client, rule: Uuid, account: &str, now: i64, n: usize) -> usize {
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
        assert!(merge_state(&ta, rule, "", &mine).await.expect("merge"));
        let second = tokio::spawn(async move {
          let tb = b.transaction().await.expect("tx");
          let theirs = obj(serde_json::json!({ "b": round }));
          let changed = merge_state(&tb, rule, "", &theirs).await.expect("merge");
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
      assert!(!merge_state(&tx, rule, "", &same).await.expect("merge"));
      let absent = obj(serde_json::json!({ "zz": null }));
      assert!(!merge_state(&tx, rule, "", &absent).await.expect("merge"));
      let gone = obj(serde_json::json!({ "a": null, "n": { "x": null } }));
      assert!(merge_state(&tx, rule, "", &gone).await.expect("merge"));
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
      let account = Uuid::nil().to_string();
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
        assert!(claim_mail(&ta, rule, &account, now).await.expect("claim"));
        let account_b = account.clone();
        let second = tokio::spawn(async move {
          let tb = b.transaction().await.expect("tx");
          let fired = apply_alert(&tb, rule, scope, "down", RuleAlertKind::Fire, now, now)
            .await
            .expect("transition");
          let mailed = fired && claim_mail(&tb, rule, &account_b, now).await.expect("claim");
          tb.commit().await.expect("commit");
          (fired, mailed)
        });
        wait_until_blocked(b_pid).await;
        ta.commit().await.expect("commit");
        assert_eq!(second.await.expect("second caller"), (false, false));
      }
      assert_eq!(mail_sent(&setup, &account, now).await, 2);
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
      let account = "acct-a";
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
      let account = "acct-b";
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
        mail_sent(&client, account, day2(0)).await,
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
        claim_mail(&ta, rule_a, "acct-c", day3)
          .await
          .expect("claim")
      );
      let second = tokio::spawn(async move {
        let tb = b.transaction().await.expect("tx");
        let claimed = claim_mail(&tb, rule_b, "acct-d", day3)
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
}
