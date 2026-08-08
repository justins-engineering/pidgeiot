use capsules::TelemetryHistoryPoint;
use time::OffsetDateTime;
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Env, Result, console_error};

use crate::helpers::PigeonAccess;
use crate::helpers::firmware::FlockAccess;
use crate::helpers::get_db_client;

/// Idempotently ensures the PG telemetry-history table + indexes exist —
/// mirrors the DO's own `CREATE TABLE IF NOT EXISTS` bootstrap pattern in
/// `objects/pigeons.rs::DurableObject::new`. Staging and production share
/// one Hyperdrive-backed Postgres with no separate migration runner, so
/// each write/read path calls this first rather than relying on a one-time
/// manual migration. Cheap no-op after the first call (`IF NOT EXISTS`).
pub async fn ensure_telemetry_history_table(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS pigeon_telemetry_history (
        id BIGSERIAL PRIMARY KEY,
        pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        value_num DOUBLE PRECISION,
        reported_at TIMESTAMPTZ NOT NULL DEFAULT now()
      );
      CREATE INDEX IF NOT EXISTS idx_pigeon_telemetry_history_pigeon_reported
        ON pigeon_telemetry_history (pigeon_id, reported_at);
      CREATE INDEX IF NOT EXISTS idx_pigeon_telemetry_history_key
        ON pigeon_telemetry_history (key);",
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history table bootstrap error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })
}

/// Best-effort PG history write for one device telemetry report, called by
/// the queue consumer (`queue.rs`) right after the DO's own latest-value
/// upsert succeeds -- matches this codebase's established best-effort PG
/// sync convention (log, never fail the primary operation). One row per
/// reported key; `value_num` is populated only when the raw string parses
/// as an `f64`, so range queries can filter numeric series without a cast
/// at query time.
pub async fn write_telemetry_history(
  env: &Env,
  pigeon_id: &str,
  metrics: &std::collections::HashMap<String, String>,
) -> Result<()> {
  let client = get_db_client(env).await?;
  ensure_telemetry_history_table(&client).await?;

  for (key, value) in metrics {
    let value_num: Option<f64> = value.parse().ok();
    client
      .execute_typed(
        "INSERT INTO pigeon_telemetry_history (pigeon_id, key, value, value_num)
         VALUES ($1, $2, $3, $4);",
        &[
          (&pigeon_id, Type::TEXT),
          (key, Type::TEXT),
          (value, Type::TEXT),
          (&value_num, Type::FLOAT8),
        ],
      )
      .await
      .map_err(|e| {
        console_error!("Telemetry history insert error for key '{key}': {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;
  }

  Ok(())
}

/// Backs `GET /pigeons/:id/telemetry/history`. Takes a `PigeonAccess` proof
/// rather than a bare `pigeon_id` -- that proof is only constructible via
/// `check_pigeon_authz` (`helpers/pigeons.rs`), which is the thing that
/// actually ACL-gates against the DO's `/pigeon/authz/check` route, so a
/// caller can no longer reach this query without having run that check
/// first (see docs/design/tenancy-isolation.md §2.1). Previously this
/// function's doc comment just asserted the caller was responsible for
/// gating; now the compiler does.
pub async fn query_telemetry_history_for_pigeon(
  client: &Client,
  access: &PigeonAccess,
  key: Option<&str>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<Vec<TelemetryHistoryPoint>> {
  ensure_telemetry_history_table(client).await?;

  let pigeon_id = access.pigeon_id();

  let rows = client
    .query_typed(
      "SELECT pigeon_id, key, value, value_num, reported_at
       FROM pigeon_telemetry_history
       WHERE pigeon_id = $1
         AND ($2::TEXT IS NULL OR key = $2)
         AND ($3::TIMESTAMPTZ IS NULL OR reported_at >= $3)
         AND ($4::TIMESTAMPTZ IS NULL OR reported_at <= $4)
       ORDER BY reported_at ASC
       LIMIT 5000;",
      &[
        (&pigeon_id, Type::TEXT),
        (&key, Type::TEXT),
        (&since, Type::TIMESTAMPTZ),
        (&until, Type::TIMESTAMPTZ),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history query error for pigeon {pigeon_id}: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .into_iter()
      .map(|row| TelemetryHistoryPoint {
        pigeon_id: row.get("pigeon_id"),
        key: row.get("key"),
        value: row.get("value"),
        value_num: row.get("value_num"),
        reported_at: row.get("reported_at"),
      })
      .collect(),
  )
}

/// Pigeon-ID list for one flock (task #26) -- the Postgres round-trip
/// `query_greptime_history_for_pigeons` (`helpers/greptime.rs`) needs
/// before it can query Greptime's SQL-over-HTTP API: Greptime has no
/// `pigeons`/`flocks` tables of its own (relational entity data, not
/// time-series). Takes a `FlockAccess` proof (task #12) rather than the
/// old fold-ownership-into-the-query `user_id` pattern -- the proof is
/// only constructible via `authorize_flock` (`helpers/orgs.rs`), which is
/// org-aware, so this query no longer needs (and must not keep) its own
/// user_id filter that org members would always fail.
pub async fn get_flock_pigeon_ids(client: &Client, access: &FlockAccess) -> Result<Vec<String>> {
  let flock_id_str = access.flock_id();
  let flock_uuid = Uuid::parse_str(flock_id_str).map_err(|e| {
    console_error!("Invalid flock_id format: {e}");
    worker::Error::RustError("Bad Request: Invalid flock_id".into())
  })?;

  let rows = client
    .query_typed(
      "SELECT p.id FROM pigeons p WHERE p.flock_id = $1;",
      &[(&flock_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock pigeon-id lookup error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.into_iter().map(|row| row.get("id")).collect())
}

/// Backs `GET /flocks/:id/telemetry/history`. Flocks have no per-entity ACL
/// table (unlike pigeons' `pigeon_acl`) -- authorization now happens in
/// `authorize_flock` (`helpers/orgs.rs`, org-aware since task #12), whose
/// passing case is the only source of the `FlockAccess` proof this
/// function requires; the old `f.user_id = $2` WHERE-clause fold is gone
/// for the same reason as `get_flock_pigeon_ids` above.
pub async fn query_telemetry_history_for_flock(
  client: &Client,
  access: &FlockAccess,
  key: Option<&str>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<Vec<TelemetryHistoryPoint>> {
  ensure_telemetry_history_table(client).await?;

  let flock_id_str = access.flock_id();
  let flock_uuid = Uuid::parse_str(flock_id_str).map_err(|e| {
    console_error!("Invalid flock_id format: {e}");
    worker::Error::RustError("Bad Request: Invalid flock_id".into())
  })?;

  let rows = client
    .query_typed(
      "SELECT h.pigeon_id, h.key, h.value, h.value_num, h.reported_at
       FROM pigeon_telemetry_history h
       JOIN pigeons p ON p.id = h.pigeon_id
       WHERE p.flock_id = $1
         AND ($2::TEXT IS NULL OR h.key = $2)
         AND ($3::TIMESTAMPTZ IS NULL OR h.reported_at >= $3)
         AND ($4::TIMESTAMPTZ IS NULL OR h.reported_at <= $4)
       ORDER BY h.reported_at ASC
       LIMIT 5000;",
      &[
        (&flock_uuid, Type::UUID),
        (&key, Type::TEXT),
        (&since, Type::TIMESTAMPTZ),
        (&until, Type::TIMESTAMPTZ),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history query error for flock {flock_id_str}: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .into_iter()
      .map(|row| TelemetryHistoryPoint {
        pigeon_id: row.get("pigeon_id"),
        key: row.get("key"),
        value: row.get("value"),
        value_num: row.get("value_num"),
        reported_at: row.get("reported_at"),
      })
      .collect(),
  )
}

/// Idempotently ensures the `pigeons.telemetry_endpoint` column exists on
/// the Postgres mirror table -- same rationale as
/// `ensure_telemetry_history_table` (no separate migration runner against
/// the shared staging/production database). Postgres, unlike SQLite,
/// supports `ADD COLUMN IF NOT EXISTS` directly, so no duplicate-column
/// error handling is needed here (contrast the DO's SQLite fallback in
/// `objects/pigeons.rs`).
pub async fn ensure_pigeons_telemetry_endpoint_column(client: &Client) -> Result<()> {
  client
    .batch_execute("ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS telemetry_endpoint JSONB;")
    .await
    .map_err(|e| {
      console_error!("pigeons.telemetry_endpoint column bootstrap error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })
}
