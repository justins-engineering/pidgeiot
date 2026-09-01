use capsules::Flock;
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Idempotently ensures `flocks.owner_email` exists -- staging/prod share
/// one Hyperdrive-backed Postgres with no separate migration runner. The
/// column is already created by `init-db.sql` and by
/// `helpers/alerts.rs::ensure_alert_tables`, so on a database that already
/// ran either this is a cheap no-op -- it's here purely so
/// `create_user_flock`/`backfill_owner_email` don't assume a migration ran
/// elsewhere first.
async fn ensure_flocks_owner_email_column(client: &Client) -> Result<()> {
  client
    .batch_execute("ALTER TABLE flocks ADD COLUMN IF NOT EXISTS owner_email TEXT;")
    .await
    .map_err(|e| {
      console_error!("flocks.owner_email column bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

/// Every flock the caller can see: personal flocks they own (`org_id IS
/// NULL AND user_id = caller`) plus every flock owned by an org they
/// belong to, any role (`member` is view-level, which listing is). The two
/// arms are mutually exclusive on purpose: once a flock is org-owned,
/// `user_id` is provenance, not an access grant (see
/// `helpers/orgs.rs::authorize_flock`).
pub async fn get_user_flocks(client: &Client, user_id_str: &str) -> Result<Vec<Flock>> {
  // Org tables/column must exist before this query references
  // flocks.org_id -- same per-request idempotent-bootstrap convention as
  // ensure_flocks_owner_email_column below.
  crate::helpers::ensure_org_tables(client).await?;

  let parsed_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid UUID format: {e}")))?;

  let rows = client
    .query_typed(
      "SELECT
        flocks.id, flocks.user_id, flocks.org_id, flocks.name, flocks.service_plan, flocks.created_at, flocks.updated_at,
        COALESCE(array_agg(pigeons.id) FILTER (WHERE pigeons.id IS NOT NULL), '{}') AS pigeon_ids
        FROM flocks
        LEFT JOIN pigeons ON pigeons.flock_id = flocks.id
        WHERE (flocks.org_id IS NULL AND flocks.user_id = $1)
           OR flocks.org_id IN (SELECT org_id FROM organization_members WHERE user_id = $1)
        GROUP BY flocks.id",
      &[(&parsed_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| Error::RustError(format!("DB Query Error: {e}")))?;

  let mut flocks = Vec::new();

  for row in rows {
    let id: Uuid = row.get("id");
    let user_id: Uuid = row.get("user_id");
    let org_id: Option<Uuid> = row.get("org_id");
    let name: String = row.get("name");
    let service_plan: String = row.get("service_plan");
    let pigeon_ids: Vec<String> = row.get("pigeon_ids");
    let updated_at: time::OffsetDateTime = row.get("updated_at");
    let created_at: time::OffsetDateTime = row.get("created_at");

    flocks.push(Flock {
      id,
      user_id,
      org_id,
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
/// `owner_email` comes straight from the caller's already-validated Kratos
/// session (`require_auth_session`'s `identity.traits.email`, `lib.rs`) --
/// it's the alerts feature's only recipient source. `None` is written
/// as-is (rather than skipping the column) so a session with no
/// resolvable email trait doesn't need a separate code path;
/// `backfill_owner_email` below picks it up later once a session does
/// carry one.
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
    // A freshly-created flock is always personal; org adoption happens
    // via the transfer route.
    org_id: None,
    name,
    service_plan,
    pigeon_ids: Vec::new(),
    updated_at,
    created_at,
  })
}

/// Deletes a flock only when it holds no pigeons. `pigeons.flock_id`
/// cascades, so an unguarded delete would take every device's mirror row
/// (and its history, firmware catalog and alerts) with it while the Durable
/// Objects lived on -- the caller deletes pigeons one at a time instead.
/// `Err` carries the user-facing 409 message, which names the count so the
/// dashboard can say how many are in the way.
///
/// The guard and the delete are one statement, but READ COMMITTED still
/// lets a pigeon created against this flock in a concurrent transaction
/// commit just after the subquery ran; that pigeon keeps its own Durable
/// Object, so what a lost race costs is its Postgres mirror row, not the
/// device.
pub async fn delete_flock_if_empty(
  client: &Client,
  flock_id: &Uuid,
) -> Result<std::result::Result<(), String>> {
  let deleted = client
    .execute_typed(
      "DELETE FROM flocks WHERE id = $1
         AND NOT EXISTS (SELECT 1 FROM pigeons WHERE pigeons.flock_id = flocks.id);",
      &[(flock_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock delete error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  if deleted > 0 {
    return Ok(Ok(()));
  }

  let rows = client
    .query_typed(
      "SELECT COUNT(*)::BIGINT AS pigeon_count FROM pigeons WHERE flock_id = $1;",
      &[(flock_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock emptiness check error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let pigeon_count = rows
    .first()
    .map(|row| row.get::<_, i64>("pigeon_count"))
    .unwrap_or_default();

  // Nothing deleted and nothing in the way means the flock is already gone.
  if pigeon_count == 0 {
    return Ok(Ok(()));
  }

  const PREFIX: &str = "Conflict: flock still holds ";
  const SUFFIX: &str = " pigeon(s) -- delete them first";
  let count = pigeon_count.to_string();
  let mut message = String::with_capacity(PREFIX.len() + count.len() + SUFFIX.len());
  message.push_str(PREFIX);
  message.push_str(&count);
  message.push_str(SUFFIX);

  Ok(Err(message))
}

/// Whether a pigeon's current flock and `dest_flock_id` answer to the same
/// owner -- the same user for two personal flocks, the same org for two
/// org-owned ones. A move across that line is refused rather than
/// half-applied: the pigeon's `pigeon_acl` rows live in its Durable Object
/// and name the old owner, so it would either vanish from the destination's
/// members or stay readable by the org it left. Moving a whole flock between
/// owners is what the transfer route is for.
///
/// `None` when the pigeon has no mirrored row, or the destination flock does
/// not exist.
pub async fn pigeon_move_shares_owner(
  client: &Client,
  pigeon_id: &str,
  dest_flock_id: &Uuid,
) -> Result<Option<bool>> {
  let rows = client
    .query_typed(
      "SELECT src.user_id AS src_user, src.org_id AS src_org,
              dst.user_id AS dst_user, dst.org_id AS dst_org
         FROM pigeons
         JOIN flocks src ON src.id = pigeons.flock_id
         JOIN flocks dst ON dst.id = $2
        WHERE pigeons.id = $1;",
      &[(&pigeon_id, Type::TEXT), (dest_flock_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Pigeon flock-move lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(|row| {
    let src_org: Option<Uuid> = row.get("src_org");
    let dst_org: Option<Uuid> = row.get("dst_org");
    if src_org.is_some() || dst_org.is_some() {
      return src_org == dst_org;
    }
    row.get::<_, Uuid>("src_user") == row.get::<_, Uuid>("dst_user")
  }))
}

/// Opportunistically fills in `owner_email` for flocks that predate this
/// column being populated on create. Chosen over a one-time backfill
/// script because there's no separate migration runner in this codebase --
/// a script would need its own deploy step and its own way to resolve each
/// owner's email from Kratos, whereas this reuses the email a session
/// already carries the next time that owner authenticates. Scoped to
/// `WHERE owner_email IS NULL` so it never clobbers an existing value, and
/// to the caller's own `user_id_str` so it can never touch another
/// tenant's flocks. Best-effort: callers log and continue on `Err` rather
/// than failing the request.
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
