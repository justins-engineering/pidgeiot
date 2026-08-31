use capsules::{DashboardStateEntry, DashboardStateEntryRow, MAX_DASHBOARD_STATE_KEYS};
use tokio_postgres::{Client, Row, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Column list shared by every `dashboard_state` read/RETURNING statement
/// -- `value` is cast to `::text` for the same reason the alert columns
/// are: this workspace's `tokio-postgres` is not built with the
/// `with-serde_json-1` feature.
const DASHBOARD_STATE_COLUMNS: &str = "scope_key, value::text AS value, updated_at";

/// Idempotently ensures the `dashboard_state` table exists -- the same
/// lazy-DDL convention as `ensure_alert_tables`, since staging and
/// production share one Hyperdrive-backed Postgres with no migration
/// runner. The primary key is also the only index the table needs: both
/// reads are a point lookup or a per-user prefix scan of it.
pub async fn ensure_dashboard_state_table(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS dashboard_state (
        user_id UUID NOT NULL,
        scope_key TEXT NOT NULL,
        value JSONB NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (user_id, scope_key)
      );",
    )
    .await
    .map_err(|e| {
      console_error!("Dashboard state table bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

fn row_to_entry(row: &Row) -> DashboardStateEntry {
  DashboardStateEntryRow {
    scope_key: row.get("scope_key"),
    value: row.get("value"),
    updated_at: row.get("updated_at"),
  }
  .into()
}

/// One account's document for a scope, or `None` when it has never saved
/// one.
///
/// `read_at` is never read back. It is there because Hyperdrive will not
/// cache a statement carrying a volatile function, and this read must not
/// be cached: a browser with no local mirror -- a fresh profile, which is
/// the whole point of storing this server-side -- has to see a save made
/// seconds ago. Same device as `load_org_billing_state`, and load-bearing
/// for the same kind of reason. Observed on staging without it: a page
/// load's 404 seeded the cache, and the next profile was served that 404
/// twenty seconds after the document was written.
pub async fn load_dashboard_state(
  client: &Client,
  user_id: &Uuid,
  scope_key: &str,
) -> Result<Option<DashboardStateEntry>> {
  ensure_dashboard_state_table(client).await?;

  let mut sql = String::with_capacity(160);
  sql.push_str("SELECT ");
  sql.push_str(DASHBOARD_STATE_COLUMNS);
  sql.push_str(", now() AS read_at FROM dashboard_state WHERE user_id = $1 AND scope_key = $2;");

  let rows = client
    .query_typed(&sql, &[(user_id, Type::UUID), (&scope_key, Type::TEXT)])
    .await
    .map_err(|e| {
      console_error!("Dashboard state read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(row_to_entry))
}

/// Replaces the document at `scope_key` wholesale, returning the stored
/// entry -- which is the only read guaranteed to reflect the write, since
/// Hyperdrive serves an identical `SELECT` from its query cache for up to
/// a minute afterwards.
///
/// `Ok(None)` means the account is at [`MAX_DASHBOARD_STATE_KEYS`] and
/// this key is new. The cap is counted inside the same statement rather
/// than by a `SELECT` first: a separate count would be a cacheable read
/// and could admit a minute's worth of new keys past the cap. Editing a
/// key that already exists is never refused, so an account at the cap can
/// still work with what it has.
pub async fn store_dashboard_state(
  client: &Client,
  user_id: &Uuid,
  scope_key: &str,
  value: &str,
) -> Result<Option<DashboardStateEntry>> {
  ensure_dashboard_state_table(client).await?;

  let mut sql = String::with_capacity(512);
  sql.push_str(
    "INSERT INTO dashboard_state (user_id, scope_key, value)
     SELECT $1, $2, $3::jsonb
     WHERE EXISTS (SELECT 1 FROM dashboard_state WHERE user_id = $1 AND scope_key = $2)
        OR (SELECT count(*) FROM dashboard_state WHERE user_id = $1) < $4
     ON CONFLICT (user_id, scope_key)
       DO UPDATE SET value = EXCLUDED.value, updated_at = now()
     RETURNING ",
  );
  sql.push_str(DASHBOARD_STATE_COLUMNS);
  sql.push(';');

  let rows = client
    .query_typed(
      &sql,
      &[
        (user_id, Type::UUID),
        (&scope_key, Type::TEXT),
        (&value, Type::TEXT),
        (&MAX_DASHBOARD_STATE_KEYS, Type::INT8),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Dashboard state write error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(row_to_entry))
}

/// Drops the document at `scope_key`. Deleting one that was never stored
/// is not an error -- the caller's intent is already satisfied.
pub async fn delete_dashboard_state(
  client: &Client,
  user_id: &Uuid,
  scope_key: &str,
) -> Result<()> {
  ensure_dashboard_state_table(client).await?;

  client
    .execute_typed(
      "DELETE FROM dashboard_state WHERE user_id = $1 AND scope_key = $2;",
      &[(user_id, Type::UUID), (&scope_key, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Dashboard state delete error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}
