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
use tokio_postgres::Client;
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
}
