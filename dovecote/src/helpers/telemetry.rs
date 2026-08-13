use capsules::{TELEMETRY_HISTORY_MAX_POINTS, TelemetryHistoryPoint};
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
///
/// Every row of one report carries the same `reported_at`, taken from the
/// report itself rather than from each statement's own arrival time. A
/// reader has no other way to tell which rows belong together: `fancier`'s
/// GPS track reassembles a fix by grouping rows on `reported_at` and drops
/// any group missing a coordinate, so letting the column default per row
/// would split a fix in two whenever the writes straddled a boundary and
/// lose it entirely. `metrics` iterates in `HashMap` order, so which keys
/// land on which side of such a split is arbitrary.
///
/// Writing all rows in a single statement is what makes one timestamp
/// natural, and collapses a round trip per key into one for the report.
pub async fn write_telemetry_history(
  env: &Env,
  pigeon_id: &str,
  metrics: &std::collections::HashMap<String, String>,
  reported_at_ms: u64,
) -> Result<()> {
  if metrics.is_empty() {
    return Ok(());
  }

  let client = get_db_client(env).await?;
  ensure_telemetry_history_table(&client).await?;

  let reported_at = OffsetDateTime::from_unix_timestamp_nanos(
    i128::from(reported_at_ms) * 1_000_000,
  )
  .map_err(|e| {
    console_error!("Telemetry history: unrepresentable reported_at {reported_at_ms}ms: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  // Owned first so the borrows handed to `execute_typed` below outlive the
  // parameter list.
  let rows: Vec<(&String, &String, Option<f64>)> = metrics
    .iter()
    .map(|(key, value)| (key, value, value.parse::<f64>().ok()))
    .collect();

  let mut params: Vec<(&(dyn tokio_postgres::types::ToSql + Sync), Type)> =
    Vec::with_capacity(rows.len() * 3 + 2);
  params.push((&pigeon_id, Type::TEXT));
  params.push((&reported_at, Type::TIMESTAMPTZ));

  let mut sql = String::from(
    "INSERT INTO pigeon_telemetry_history (pigeon_id, key, value, value_num, reported_at) VALUES ",
  );
  for (i, (key, value, value_num)) in rows.iter().enumerate() {
    if i > 0 {
      sql.push(',');
    }
    let base = i * 3 + 3;
    sql.push_str(&format!(
      "($1, ${}, ${}, ${}, $2)",
      base,
      base + 1,
      base + 2
    ));
    params.push((*key, Type::TEXT));
    params.push((*value, Type::TEXT));
    params.push((value_num, Type::FLOAT8));
  }
  sql.push(';');

  client.execute_typed(&sql, &params).await.map_err(|e| {
    console_error!("Telemetry history insert error for pigeon {pigeon_id}: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  Ok(())
}

/// One history read's worth of points, plus whether the range held more
/// than `TELEMETRY_HISTORY_MAX_POINTS` and was cut down to its newest
/// slice. A chart drawn from a silently cut range misreads as a complete
/// one, so the flag rides alongside the points rather than being inferred
/// from their count.
pub struct TelemetryHistoryPage {
  pub points: Vec<TelemetryHistoryPoint>,
  pub truncated: bool,
}

impl TelemetryHistoryPage {
  /// Takes the newest `TELEMETRY_HISTORY_MAX_POINTS` of an
  /// ascending-by-time slice, which is the end a chart actually plots --
  /// dropping from the front, since the excess is at the old end.
  pub(crate) fn from_ascending(mut points: Vec<TelemetryHistoryPoint>) -> Self {
    let truncated = points.len() > TELEMETRY_HISTORY_MAX_POINTS;
    if truncated {
      points.drain(..points.len() - TELEMETRY_HISTORY_MAX_POINTS);
    }
    Self { points, truncated }
  }
}

/// One more row than the cap, so a full page can be told apart from a
/// range that merely ends exactly on it.
fn history_probe_limit() -> i64 {
  TELEMETRY_HISTORY_MAX_POINTS as i64 + 1
}

/// Backs `GET /pigeons/:id/telemetry/history`. Takes a `PigeonAccess` proof
/// rather than a bare `pigeon_id` -- only constructible via
/// `check_pigeon_authz` (`helpers/pigeons.rs`), which ACL-gates against
/// the DO's `/pigeon/authz/check` route, so a caller can't reach this
/// query without that check having already run.
///
/// The cap is applied to the newest end of the range and the result handed
/// back oldest-first. Selecting the oldest rows instead would drop the
/// live edge -- the part of a range a chart exists to show -- and a few
/// keys reported every minute exceed the cap inside a day, so even a
/// day-long range depends on which end is kept.
pub async fn query_telemetry_history_for_pigeon(
  client: &Client,
  access: &PigeonAccess,
  keys: Option<&[String]>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<TelemetryHistoryPage> {
  ensure_telemetry_history_table(client).await?;

  let pigeon_id = access.pigeon_id();
  let limit = history_probe_limit();

  let rows = client
    .query_typed(
      "SELECT pigeon_id, key, value, value_num, reported_at FROM (
         SELECT pigeon_id, key, value, value_num, reported_at
         FROM pigeon_telemetry_history
         WHERE pigeon_id = $1
           AND ($2::TEXT[] IS NULL OR key = ANY($2))
           AND ($3::TIMESTAMPTZ IS NULL OR reported_at >= $3)
           AND ($4::TIMESTAMPTZ IS NULL OR reported_at <= $4)
         ORDER BY reported_at DESC
         LIMIT $5
       ) newest
       ORDER BY reported_at ASC;",
      &[
        (&pigeon_id, Type::TEXT),
        (&keys, Type::TEXT_ARRAY),
        (&since, Type::TIMESTAMPTZ),
        (&until, Type::TIMESTAMPTZ),
        (&limit, Type::INT8),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history query error for pigeon {pigeon_id}: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(TelemetryHistoryPage::from_ascending(
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
  ))
}

/// Pigeon-ID list for one flock -- the Postgres round-trip
/// `query_greptime_history_for_pigeons` (`helpers/greptime.rs`) needs
/// before it can query Greptime's SQL-over-HTTP API, since Greptime has no
/// `pigeons`/`flocks` tables of its own (relational entity data, not
/// time-series). Takes a `FlockAccess` proof, constructible only via the
/// org-aware `authorize_flock` (`helpers/orgs.rs`) -- must not gain its
/// own `user_id` filter, which would reject org members who aren't the
/// flock's original owner.
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
/// table (unlike pigeons' `pigeon_acl`) -- authorization happens in
/// `authorize_flock` (`helpers/orgs.rs`), whose passing case is the only
/// source of the `FlockAccess` proof this function requires.
pub async fn query_telemetry_history_for_flock(
  client: &Client,
  access: &FlockAccess,
  keys: Option<&[String]>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<TelemetryHistoryPage> {
  ensure_telemetry_history_table(client).await?;

  let flock_id_str = access.flock_id();
  let flock_uuid = Uuid::parse_str(flock_id_str).map_err(|e| {
    console_error!("Invalid flock_id format: {e}");
    worker::Error::RustError("Bad Request: Invalid flock_id".into())
  })?;
  let limit = history_probe_limit();

  let rows = client
    .query_typed(
      "SELECT pigeon_id, key, value, value_num, reported_at FROM (
         SELECT h.pigeon_id, h.key, h.value, h.value_num, h.reported_at
         FROM pigeon_telemetry_history h
         JOIN pigeons p ON p.id = h.pigeon_id
         WHERE p.flock_id = $1
           AND ($2::TEXT[] IS NULL OR h.key = ANY($2))
           AND ($3::TIMESTAMPTZ IS NULL OR h.reported_at >= $3)
           AND ($4::TIMESTAMPTZ IS NULL OR h.reported_at <= $4)
         ORDER BY h.reported_at DESC
         LIMIT $5
       ) newest
       ORDER BY reported_at ASC;",
      &[
        (&flock_uuid, Type::UUID),
        (&keys, Type::TEXT_ARRAY),
        (&since, Type::TIMESTAMPTZ),
        (&until, Type::TIMESTAMPTZ),
        (&limit, Type::INT8),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history query error for flock {flock_id_str}: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(TelemetryHistoryPage::from_ascending(
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
  ))
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
