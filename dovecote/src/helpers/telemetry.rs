use capsules::TelemetryHistoryPoint;
use tokio_postgres::{Client, types::Type};
use time::OffsetDateTime;
use uuid::Uuid;
use worker::{Env, Result, console_error};

use crate::helpers::PigeonAccess;
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

/// Pigeon-ID list for one flock, scoped by ownership (task #26) -- the
/// Postgres round-trip `query_greptime_history_for_pigeons`
/// (`helpers/greptime.rs`) needs before it can query Greptime's
/// SQL-over-HTTP API: Greptime has no `pigeons`/`flocks` tables of its own
/// (relational entity data, not time-series), so "which pigeon IDs belong
/// to this flock, and does this user actually own it" can only be answered
/// from Postgres. Same "fold ownership into the query" pattern as
/// `query_telemetry_history_for_flock` below -- a flock this user doesn't
/// own returns an empty list, not a 403.
pub async fn get_flock_pigeon_ids(
  client: &Client,
  flock_id_str: &str,
  user_id_str: &str,
) -> Result<Vec<String>> {
  let flock_uuid = Uuid::parse_str(flock_id_str).map_err(|e| {
    console_error!("Invalid flock_id format: {e}");
    worker::Error::RustError("Bad Request: Invalid flock_id".into())
  })?;
  let user_uuid = Uuid::parse_str(user_id_str).map_err(|e| {
    console_error!("Invalid X-User-Id format: {e}");
    worker::Error::RustError("Bad Request: Invalid X-User-Id".into())
  })?;

  let rows = client
    .query_typed(
      "SELECT p.id FROM pigeons p
       JOIN flocks f ON f.id = p.flock_id
       WHERE f.id = $1 AND f.user_id = $2;",
      &[(&flock_uuid, Type::UUID), (&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock pigeon-id lookup error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.into_iter().map(|row| row.get("id")).collect())
}

/// Backs `GET /flocks/:id/telemetry/history`. Flocks have no per-entity ACL
/// table (unlike pigeons' `pigeon_acl`) -- ownership is the single
/// `flocks.user_id` column (see `helpers/flocks.rs::get_user_flocks`), so
/// authorization is folded directly into the query's WHERE clause: a flock
/// that isn't owned by `user_id_str` simply yields zero rows rather than a
/// separate 403 path.
pub async fn query_telemetry_history_for_flock(
  client: &Client,
  flock_id_str: &str,
  user_id_str: &str,
  key: Option<&str>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<Vec<TelemetryHistoryPoint>> {
  ensure_telemetry_history_table(client).await?;

  let flock_uuid = Uuid::parse_str(flock_id_str).map_err(|e| {
    console_error!("Invalid flock_id format: {e}");
    worker::Error::RustError("Bad Request: Invalid flock_id".into())
  })?;
  let user_uuid = Uuid::parse_str(user_id_str).map_err(|e| {
    console_error!("Invalid X-User-Id format: {e}");
    worker::Error::RustError("Bad Request: Invalid X-User-Id".into())
  })?;

  let rows = client
    .query_typed(
      "SELECT h.pigeon_id, h.key, h.value, h.value_num, h.reported_at
       FROM pigeon_telemetry_history h
       JOIN pigeons p ON p.id = h.pigeon_id
       JOIN flocks f ON f.id = p.flock_id
       WHERE f.id = $1 AND f.user_id = $2
         AND ($3::TEXT IS NULL OR h.key = $3)
         AND ($4::TIMESTAMPTZ IS NULL OR h.reported_at >= $4)
         AND ($5::TIMESTAMPTZ IS NULL OR h.reported_at <= $5)
       ORDER BY h.reported_at ASC
       LIMIT 5000;",
      &[
        (&flock_uuid, Type::UUID),
        (&user_uuid, Type::UUID),
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
