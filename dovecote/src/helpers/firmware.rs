use capsules::FirmwareImage;
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Idempotently ensures the PG firmware catalog table + index exist —
/// mirrors `ensure_telemetry_history_table`'s rationale (task #18):
/// staging and production share one Hyperdrive-backed Postgres with no
/// separate migration runner, so each read/write path calls this first
/// rather than relying on a one-time manual migration against an
/// already-deployed database. Cheap no-op after the first call (`IF NOT
/// EXISTS`).
pub async fn ensure_flock_firmware_table(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS flock_firmware (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        flock_id UUID NOT NULL REFERENCES flocks(id) ON DELETE CASCADE,
        version TEXT NOT NULL,
        size BIGINT NOT NULL,
        sha256 TEXT NOT NULL,
        uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        UNIQUE (flock_id, sha256)
      );
      CREATE INDEX IF NOT EXISTS idx_flock_firmware_flock_id ON flock_firmware(flock_id);",
    )
    .await
    .map_err(|e| {
      console_error!("flock_firmware table bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  // Idempotent for pre-existing databases that created `flock_firmware`
  // before this column existed (task #20, phase 1) -- same
  // no-separate-migration-runner rationale as `telemetry_endpoint`
  // (`helpers/telemetry.rs::ensure_pigeons_telemetry_endpoint_column`).
  // Nullable at the schema level so pre-existing rows don't need a
  // backfill -- the upload route requires `board` for every NEW image
  // (see `lib.rs`'s `POST /flocks/:flock_id/firmware`), but an old,
  // already-uploaded image simply stays untagged (and, under the
  // fail-closed board-compatibility check in `objects/pigeons.rs`,
  // unassignable) until an operator tags it.
  client
    .batch_execute("ALTER TABLE flock_firmware ADD COLUMN IF NOT EXISTS board TEXT;")
    .await
    .map_err(|e| {
      console_error!("flock_firmware.board column bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

/// Firmware images are shared across every pigeon in a flock (the same
/// hardware fleet), not scoped per-pigeon, so ownership is checked
/// directly against `flocks.user_id` — the same "fold ownership into the
/// query" model `query_telemetry_history_for_flock` (`helpers/telemetry.rs`)
/// uses, just returned as a bool here since the upload/list routes need an
/// explicit 403 rather than silently-empty results.
pub async fn is_flock_owner(
  client: &Client,
  flock_id_str: &str,
  user_id_str: &str,
) -> Result<bool> {
  let flock_uuid = Uuid::parse_str(flock_id_str)
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;
  let user_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid X-User-Id format: {e}")))?;

  let row = client
    .query_typed_one(
      "SELECT EXISTS(SELECT 1 FROM flocks WHERE id = $1 AND user_id = $2) AS exists_flag",
      &[(&flock_uuid, Type::UUID), (&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock ownership check query error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(row.get::<_, bool>("exists_flag"))
}

/// Content-addressed by `(flock_id, sha256)`: re-uploading the same binary
/// to the same flock (even under a different `version` label) updates the
/// existing catalog row in place rather than creating a duplicate — the R2
/// object itself is already deduplicated the same way (see the gateway
/// route in `lib.rs`, which always writes to the same `firmware/<sha256>.bin`
/// key regardless of how many times it's uploaded).
pub async fn upsert_flock_firmware(
  client: &Client,
  flock_id_str: &str,
  version: &str,
  size: i64,
  sha256: &str,
  board: &str,
) -> Result<FirmwareImage> {
  ensure_flock_firmware_table(client).await?;

  let flock_uuid = Uuid::parse_str(flock_id_str)
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;

  let row = client
    .query_typed_one(
      "INSERT INTO flock_firmware (flock_id, version, size, sha256, board)
       VALUES ($1, $2, $3, $4, $5)
       ON CONFLICT (flock_id, sha256) DO UPDATE SET
         version = EXCLUDED.version,
         board = EXCLUDED.board,
         uploaded_at = now()
       RETURNING id, flock_id, version, size, sha256, board, uploaded_at;",
      &[
        (&flock_uuid, Type::UUID),
        (&version, Type::TEXT),
        (&size, Type::INT8),
        (&sha256, Type::TEXT),
        (&board, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Firmware catalog insert error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(FirmwareImage {
    id: row.get("id"),
    flock_id: row.get("flock_id"),
    version: row.get("version"),
    size: row.get("size"),
    sha256: row.get("sha256"),
    board: row.get("board"),
    uploaded_at: row.get("uploaded_at"),
  })
}

/// Backs `GET /flocks/:flock_id/firmware`. Caller is responsible for
/// owner-gating (`is_flock_owner`) before this runs — mirrors
/// `query_telemetry_history_for_pigeon`'s convention of trusting
/// `flock_id_str` unconditionally once authorization has already been
/// checked.
pub async fn list_flock_firmware(
  client: &Client,
  flock_id_str: &str,
) -> Result<Vec<FirmwareImage>> {
  ensure_flock_firmware_table(client).await?;

  let flock_uuid = Uuid::parse_str(flock_id_str)
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;

  let rows = client
    .query_typed(
      "SELECT id, flock_id, version, size, sha256, board, uploaded_at
       FROM flock_firmware WHERE flock_id = $1 ORDER BY uploaded_at DESC;",
      &[(&flock_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Firmware catalog list error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .into_iter()
      .map(|row| FirmwareImage {
        id: row.get("id"),
        flock_id: row.get("flock_id"),
        version: row.get("version"),
        size: row.get("size"),
        sha256: row.get("sha256"),
        board: row.get("board"),
        uploaded_at: row.get("uploaded_at"),
      })
      .collect(),
  )
}

/// Board declared for one flock's catalog image, looked up by content hash
/// -- the enforcement lookup behind `objects/pigeons.rs::
/// check_firmware_board_compat` (task #20, phase 1's fail-closed
/// board-compatibility check). Returns `Ok(None)` both when the column is
/// genuinely unset (pre-migration/untagged image) and when no catalog row
/// matches at all (e.g. a stale/foreign sha256 not in this flock's
/// catalog) -- the caller's fail-closed rule treats both the same way
/// (reject), so this function doesn't need to distinguish them; a caller
/// that does need to tell the two apart should use `list_flock_firmware`
/// instead.
pub async fn get_firmware_board(
  client: &Client,
  flock_id_str: &str,
  sha256: &str,
) -> Result<Option<String>> {
  ensure_flock_firmware_table(client).await?;

  let flock_uuid = Uuid::parse_str(flock_id_str)
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;

  let rows = client
    .query_typed(
      "SELECT board FROM flock_firmware WHERE flock_id = $1 AND sha256 = $2;",
      &[(&flock_uuid, Type::UUID), (&sha256, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Firmware board lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.into_iter().next().and_then(|row| row.get("board")))
}

/// Hex-encoded (lowercase) SHA-256 of `bytes`, computed server-side — the
/// upload route never trusts a client-supplied hash, so the catalog's
/// `sha256` and the R2 object key (`firmware/<sha256>.bin`) are always
/// correct by construction. Hex, not base64, to match what the device side
/// (mbedTLS/PSA sha256) naturally hex-compares — see
/// `capsules::FirmwareTarget`'s doc comment.
pub fn sha256_hex(bytes: &[u8]) -> String {
  use sha2::{Digest, Sha256};
  Sha256::digest(bytes)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect()
}
