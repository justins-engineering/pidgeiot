use capsules::Flock;
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Idempotently ensures `flocks.owner_email` exists -- mirrors
/// `ensure_flock_firmware_table`'s (`helpers/firmware.rs`) "ALTER TABLE ...
/// ADD COLUMN IF NOT EXISTS" rationale: staging/prod share one
/// Hyperdrive-backed Postgres with no separate migration runner. The
/// column is already created by `init-db.sql` and by
/// `helpers/alerts.rs::ensure_alert_tables` (task #32), so on any database
/// that already ran either of those this is a cheap no-op -- it's here
/// purely so `create_user_flock`/`backfill_owner_email` don't assume a
/// migration ran elsewhere first.
async fn ensure_flocks_owner_email_column(client: &Client) -> Result<()> {
  client
    .batch_execute("ALTER TABLE flocks ADD COLUMN IF NOT EXISTS owner_email TEXT;")
    .await
    .map_err(|e| {
      console_error!("flocks.owner_email column bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

pub async fn get_user_flocks(client: &Client, user_id_str: &str) -> Result<Vec<Flock>> {
  let parsed_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid UUID format: {e}")))?;

  let rows = client
    .query_typed(
      "SELECT
        flocks.id, flocks.user_id, flocks.name, flocks.service_plan, flocks.created_at, flocks.updated_at,
        COALESCE(array_agg(pigeons.id) FILTER (WHERE pigeons.id IS NOT NULL), '{}') AS pigeon_ids
        FROM flocks
        LEFT JOIN pigeons ON pigeons.flock_id = flocks.id
        WHERE flocks.user_id = $1
        GROUP BY flocks.id",
      &[(&parsed_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| Error::RustError(format!("DB Query Error: {e}")))?;

  let mut flocks = Vec::new();

  for row in rows {
    let id: Uuid = row.get("id");
    let user_id: Uuid = row.get("user_id");
    let name: String = row.get("name");
    let service_plan: String = row.get("service_plan");
    let pigeon_ids: Vec<String> = row.get("pigeon_ids");
    let updated_at: time::OffsetDateTime = row.get("updated_at");
    let created_at: time::OffsetDateTime = row.get("created_at");

    flocks.push(Flock {
      id,
      user_id,
      name,
      service_plan,
      pigeon_ids,
      updated_at,
      created_at,
    });
  }

  Ok(flocks)
}

/// Inserts a new flock into the database and returns the fully populated record.
///
/// `owner_email` (design doc `docs/design/alerts-triggers.md` §3.4) comes
/// straight from the caller's already-validated Kratos session
/// (`require_auth_session`'s `identity.traits.email`, `lib.rs`) -- this is
/// the cheapest hook the doc identifies for populating
/// `flocks.owner_email`, the alerts feature's only recipient source. `None`
/// is written as-is (rather than skipping the column) so a flock created by
/// a session that, unusually, had no resolvable email trait doesn't need a
/// separate code path; `backfill_owner_email` below will pick it up later
/// once a session does carry one.
pub async fn create_user_flock(
  client: &Client,
  user_id_str: &str,
  flock_name: &str,
  owner_email: Option<&str>,
) -> Result<Flock> {
  ensure_flocks_owner_email_column(client).await?;

  let parsed_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid UUID format: {e}")))?;

  let row = client
    .query_typed_one(
      "INSERT INTO flocks (user_id, name, service_plan, owner_email)
       VALUES ($1, $2, 'free', $3)
       RETURNING id, user_id, name, service_plan, created_at, updated_at",
      &[
        (&parsed_uuid, Type::UUID),
        (&flock_name, Type::TEXT),
        (&owner_email, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| Error::RustError(format!("Failed to insert flock: {e}")))?;

  let id: Uuid = row.get("id");
  let user_id: Uuid = row.get("user_id");
  let name: String = row.get("name");
  let service_plan: String = row.get("service_plan");
  let updated_at: time::OffsetDateTime = row.get("updated_at");
  let created_at: time::OffsetDateTime = row.get("created_at");

  Ok(Flock {
    id,
    user_id,
    name,
    service_plan,
    pigeon_ids: Vec::new(),
    updated_at,
    created_at,
  })
}

/// Opportunistically fills in `owner_email` for flocks that predate this
/// column being populated on create (design doc §3.4's "existing flocks
/// aren't stuck without a recipient" concern). Chosen over a one-time
/// backfill script because there's no separate migration runner in this
/// codebase (same reasoning as every other runtime `ensure_*`/idempotent
/// `ALTER` here) -- a script would need its own deploy step and its own way
/// to resolve each owner's email from Kratos, whereas this reuses the email
/// a session already carries the next time that owner authenticates.
/// Scoped to `WHERE owner_email IS NULL` so it never clobbers a value set
/// at creation time or by a prior run of this same backfill, and only ever
/// touches flocks owned by the caller (`user_id_str`) -- never a
/// cross-tenant write. Intentionally best-effort: callers (the
/// authenticated `GET /flocks` route) log and continue on `Err` rather than
/// failing the request, matching this codebase's universal PG-sync
/// convention.
pub async fn backfill_owner_email(client: &Client, user_id_str: &str, email: &str) -> Result<()> {
  ensure_flocks_owner_email_column(client).await?;

  let parsed_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid UUID format: {e}")))?;

  client
    .execute_typed(
      "UPDATE flocks SET owner_email = $1 WHERE user_id = $2 AND owner_email IS NULL;",
      &[(&email, Type::TEXT), (&parsed_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| Error::RustError(format!("Failed to backfill owner_email: {e}")))?;

  Ok(())
}
