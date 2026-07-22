use crate::helpers::{FlockAccess, PigeonAccess, get_db_client};
use capsules::{
  AlertChannel, AlertCondition, AlertDefinition, AlertDefinitionRow, AlertDefinitionUpdateRequest,
  AlertScope, AlertStatus,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use tokio_postgres::{Client, Row, types::Type};
use uuid::Uuid;
use worker::{Env, Error, Fetch, Method, Request, RequestInit, Result, console_error};

/// Column list shared by every `alert_definitions` read/RETURNING statement
/// -- `condition`/`channel` are cast to `::text` here rather than read as
/// native JSONB because this workspace's `tokio-postgres` dependency isn't
/// built with the `with-serde_json-1` feature (see `Cargo.toml`); every
/// other JSONB column in this codebase is only ever written, never read
/// back through `tokio-postgres` directly (shadow/connector state is always
/// read via the DO's SQLite, which stores it as plain `TEXT`), so this cast
/// is new but mirrors exactly how those columns are written ($N::jsonb) --
/// just the read-side mirror of that pattern.
const ALERT_DEFINITION_COLUMNS: &str = "id, user_id, flock_id, pigeon_id, name, \
  condition::text AS condition, severity, channel::text AS channel, enabled, \
  created_at, updated_at";

/// Fixed debounce window before a continuously-true condition transitions
/// `Ok -> Firing` (design doc §2.3). The doc's own recommendation is to
/// scale this per-pigeon off `telemetry_interval` the same way
/// `fancier::helpers::connection_state::classify` already does -- that
/// requires moving `classify` into this crate first (design doc §1.3),
/// which is out of scope for this backend-foundation task. A single fixed
/// window is a deliberate, documented simplification versus that
/// recommendation, not an oversight -- see this module's top-level doc
/// comment.
const ALERT_DEBOUNCE_SECS: i64 = 60;

/// `From:` address for alert emails sent via Resend (design doc §3.2/§3.3)
/// -- shares the platform's one verified sending domain with task #33's
/// Kratos courier setup, but never the credential (separate
/// `RESEND_API_KEY` secret, see `resend_api_key` below).
const RESEND_FROM_ADDRESS: &str = "alerts@pidgeiot.com";

/// Idempotently ensures the `alert_definitions`/`alert_state` tables (+
/// indexes) exist -- mirrors `ensure_telemetry_history_table`/
/// `ensure_flock_firmware_table`'s rationale: staging and production share
/// one Hyperdrive-backed Postgres with no separate migration runner.
/// Deliberately does NOT (re-)create the `updated_at` trigger `init-db.sql`
/// sets up for a fresh database -- `CREATE TRIGGER` has no `IF NOT EXISTS`
/// guard on the Postgres version this project targets, so every other
/// runtime `ensure_*` helper in this codebase already avoids creating
/// triggers for exactly this reason. `update_alert_definition` below sets
/// `updated_at = now()` explicitly in its own `UPDATE`, so behavior is
/// correct whether or not the trigger exists on a given database.
pub async fn ensure_alert_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS alert_definitions (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        user_id UUID NOT NULL,
        flock_id UUID REFERENCES flocks(id) ON DELETE CASCADE,
        pigeon_id TEXT REFERENCES pigeons(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        condition JSONB NOT NULL,
        severity TEXT NOT NULL DEFAULT 'warning',
        channel JSONB NOT NULL,
        enabled BOOLEAN NOT NULL DEFAULT true,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        CONSTRAINT alert_definitions_scope_check CHECK (
          (flock_id IS NOT NULL AND pigeon_id IS NULL) OR
          (flock_id IS NULL AND pigeon_id IS NOT NULL)
        )
      );
      CREATE INDEX IF NOT EXISTS idx_alert_definitions_pigeon ON alert_definitions(pigeon_id) WHERE pigeon_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS idx_alert_definitions_flock ON alert_definitions(flock_id) WHERE flock_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS idx_alert_definitions_user_id ON alert_definitions(user_id);
      CREATE TABLE IF NOT EXISTS alert_state (
        alert_definition_id UUID NOT NULL REFERENCES alert_definitions(id) ON DELETE CASCADE,
        pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
        status TEXT NOT NULL DEFAULT 'ok',
        first_true_at TIMESTAMPTZ,
        last_notified_at TIMESTAMPTZ,
        PRIMARY KEY (alert_definition_id, pigeon_id)
      );
      ALTER TABLE flocks ADD COLUMN IF NOT EXISTS owner_email TEXT;",
    )
    .await
    .map_err(|e| {
      console_error!("Alert tables bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

fn row_to_alert_definition_row(row: &Row) -> AlertDefinitionRow {
  AlertDefinitionRow {
    id: row.get("id"),
    user_id: row.get("user_id"),
    flock_id: row.get("flock_id"),
    pigeon_id: row.get("pigeon_id"),
    name: row.get("name"),
    condition: row.get("condition"),
    severity: row.get("severity"),
    channel: row.get("channel"),
    enabled: row.get("enabled"),
    created_at: row.get("created_at"),
    updated_at: row.get("updated_at"),
  }
}

/// Proof that `is_alert_owner` already confirmed the requesting user owns
/// this alert definition (`alert_definitions.user_id`) -- same
/// "caller must have already checked" guard as `PigeonAccess`/`FlockAccess`
/// (task #36 pattern -- see docs/design/tenancy-isolation.md §2.1), applied
/// to alert ownership.
pub struct AlertAccess {
  alert_id: Uuid,
}

impl AlertAccess {
  pub fn alert_id(&self) -> Uuid {
    self.alert_id
  }
}

/// Ownership check backing `PUT`/`DELETE /alerts/:alert_id` -- an alert
/// definition's owner is whoever created it (`alert_definitions.user_id`),
/// regardless of whether it's pigeon- or flock-scoped, so this is a single
/// direct check rather than re-resolving pigeon ACL or flock ownership.
pub async fn is_alert_owner(
  client: &Client,
  alert_id_str: &str,
  user_id_str: &str,
) -> Result<Option<AlertAccess>> {
  ensure_alert_tables(client).await?;

  let alert_uuid = Uuid::parse_str(alert_id_str)
    .map_err(|e| Error::RustError(format!("Invalid alert_id format: {e}")))?;
  let user_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid X-User-Id format: {e}")))?;

  let row = client
    .query_typed_one(
      "SELECT EXISTS(SELECT 1 FROM alert_definitions WHERE id = $1 AND user_id = $2) AS exists_flag",
      &[(&alert_uuid, Type::UUID), (&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert ownership check query error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(row.get::<_, bool>("exists_flag").then_some(AlertAccess {
    alert_id: alert_uuid,
  }))
}

/// Creates a pigeon-scoped alert definition. Takes a `PigeonAccess` proof
/// (not a bare `pigeon_id`) -- only constructible via `check_pigeon_authz`,
/// which is what actually confirmed this user can act on this pigeon (see
/// docs/design/tenancy-isolation.md §2.1) -- same guard
/// `query_telemetry_history_for_pigeon` already requires.
pub async fn create_pigeon_alert(
  client: &Client,
  access: &PigeonAccess,
  user_id_str: &str,
  req: &capsules::AlertDefinitionCreateRequest,
) -> Result<AlertDefinition> {
  ensure_alert_tables(client).await?;

  let user_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid X-User-Id format: {e}")))?;
  let pigeon_id = access.pigeon_id();
  let condition_json = serde_json::to_string(&req.condition).unwrap_or_else(|_| "{}".to_string());
  let channel_json = serde_json::to_string(&req.channel).unwrap_or_else(|_| "{}".to_string());
  let severity_str = req.severity.as_str();

  let row = client
    .query_typed_one(
      &format!(
        "INSERT INTO alert_definitions (user_id, pigeon_id, name, condition, severity, channel)
         VALUES ($1, $2, $3, $4::jsonb, $5, $6::jsonb)
         RETURNING {ALERT_DEFINITION_COLUMNS};"
      ),
      &[
        (&user_uuid, Type::UUID),
        (&pigeon_id, Type::TEXT),
        (&req.name, Type::TEXT),
        (&condition_json, Type::TEXT),
        (&severity_str, Type::TEXT),
        (&channel_json, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition insert error (pigeon scope): {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(AlertDefinition::from(row_to_alert_definition_row(&row)))
}

/// Creates a flock-scoped alert definition. Takes a `FlockAccess` proof --
/// same "caller must have already checked" guard as `list_flock_firmware`
/// (`helpers/firmware.rs`), applied here to alert creation.
pub async fn create_flock_alert(
  client: &Client,
  access: &FlockAccess,
  user_id_str: &str,
  req: &capsules::AlertDefinitionCreateRequest,
) -> Result<AlertDefinition> {
  ensure_alert_tables(client).await?;

  let user_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid X-User-Id format: {e}")))?;
  let flock_uuid = Uuid::parse_str(access.flock_id())
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;
  let condition_json = serde_json::to_string(&req.condition).unwrap_or_else(|_| "{}".to_string());
  let channel_json = serde_json::to_string(&req.channel).unwrap_or_else(|_| "{}".to_string());
  let severity_str = req.severity.as_str();

  let row = client
    .query_typed_one(
      &format!(
        "INSERT INTO alert_definitions (user_id, flock_id, name, condition, severity, channel)
         VALUES ($1, $2, $3, $4::jsonb, $5, $6::jsonb)
         RETURNING {ALERT_DEFINITION_COLUMNS};"
      ),
      &[
        (&user_uuid, Type::UUID),
        (&flock_uuid, Type::UUID),
        (&req.name, Type::TEXT),
        (&condition_json, Type::TEXT),
        (&severity_str, Type::TEXT),
        (&channel_json, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition insert error (flock scope): {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(AlertDefinition::from(row_to_alert_definition_row(&row)))
}

/// Backs `GET /pigeons/:pigeon_id/alerts`. Only returns alerts scoped
/// directly to this pigeon -- flock-scoped alerts covering this pigeon are
/// not inlined here (kept simple for this backend-foundation slice; the
/// dashboard's flock-level alerts tab, design doc §4, is the place a
/// flock-scoped alert is expected to show up).
pub async fn list_pigeon_alerts(
  client: &Client,
  access: &PigeonAccess,
) -> Result<Vec<AlertDefinition>> {
  ensure_alert_tables(client).await?;

  let pigeon_id = access.pigeon_id();
  let rows = client
    .query_typed(
      &format!(
        "SELECT {ALERT_DEFINITION_COLUMNS} FROM alert_definitions WHERE pigeon_id = $1 ORDER BY created_at DESC;"
      ),
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition list error (pigeon scope): {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .iter()
      .map(row_to_alert_definition_row)
      .map(AlertDefinition::from)
      .collect(),
  )
}

/// Backs `GET /flocks/:flock_id/alerts`.
pub async fn list_flock_alerts(
  client: &Client,
  access: &FlockAccess,
) -> Result<Vec<AlertDefinition>> {
  ensure_alert_tables(client).await?;

  let flock_uuid = Uuid::parse_str(access.flock_id())
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;
  let rows = client
    .query_typed(
      &format!(
        "SELECT {ALERT_DEFINITION_COLUMNS} FROM alert_definitions WHERE flock_id = $1 ORDER BY created_at DESC;"
      ),
      &[(&flock_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition list error (flock scope): {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .iter()
      .map(row_to_alert_definition_row)
      .map(AlertDefinition::from)
      .collect(),
  )
}

/// Backs `PUT /alerts/:alert_id` -- `COALESCE`/partial-update semantics,
/// same convention as `PigeonUpdateRequest`'s DO-side handler: an omitted
/// field keeps its current value.
pub async fn update_alert_definition(
  client: &Client,
  access: &AlertAccess,
  req: &AlertDefinitionUpdateRequest,
) -> Result<AlertDefinition> {
  ensure_alert_tables(client).await?;

  let condition_json = req
    .condition
    .as_ref()
    .map(|c| serde_json::to_string(c).unwrap_or_else(|_| "{}".to_string()));
  let channel_json = req
    .channel
    .as_ref()
    .map(|c| serde_json::to_string(c).unwrap_or_else(|_| "{}".to_string()));
  let severity_str = req.severity.map(|s| s.as_str().to_string());
  let alert_id = access.alert_id();

  let row = client
    .query_typed_one(
      &format!(
        "UPDATE alert_definitions SET
           name = COALESCE($2, name),
           condition = COALESCE($3::jsonb, condition),
           severity = COALESCE($4, severity),
           channel = COALESCE($5::jsonb, channel),
           enabled = COALESCE($6, enabled),
           updated_at = now()
         WHERE id = $1
         RETURNING {ALERT_DEFINITION_COLUMNS};"
      ),
      &[
        (&alert_id, Type::UUID),
        (&req.name, Type::TEXT),
        (&condition_json, Type::TEXT),
        (&severity_str, Type::TEXT),
        (&channel_json, Type::TEXT),
        (&req.enabled, Type::BOOL),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition update error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(AlertDefinition::from(row_to_alert_definition_row(&row)))
}

/// Backs `DELETE /alerts/:alert_id`. `alert_state` rows cascade via the
/// table's own `ON DELETE CASCADE` FK.
pub async fn delete_alert_definition(client: &Client, access: &AlertAccess) -> Result<()> {
  ensure_alert_tables(client).await?;

  client
    .execute_typed(
      "DELETE FROM alert_definitions WHERE id = $1;",
      &[(&access.alert_id(), Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition delete error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}

/// Evaluation hook (design doc §2.2) -- called alongside
/// `write_telemetry_default` at each of its three call sites
/// (`queue.rs::dispatch_to_do`, `objects/pigeons.rs::handle_ws_telemetry`/
/// `report_telemetry_device`), NOT only from `queue.rs` -- the design doc's
/// own audit found `queue.rs` alone misses dev entirely (no queue bound)
/// and always misses WS-telemetry. Best-effort: every failure is logged,
/// never propagated to fail the caller's own (already-succeeded) primary
/// write, matching this codebase's universal cross-store-sync convention.
///
/// Resolves every enabled alert definition scoped either directly to this
/// pigeon or to the flock it belongs to (one query, via a LEFT JOIN against
/// `pigeons` rather than a second round trip to resolve `flock_id`
/// first -- this DO already has Hyperdrive access at this exact point in
/// the request lifecycle, per the design doc's own confirmation). Only
/// `Threshold` conditions are evaluated here -- see `AlertCondition`'s doc
/// comment in `capsules` for why `DeviceState` is a no-op in this hook.
pub async fn check_telemetry_alerts(
  env: &Env,
  pigeon_id: &str,
  metrics: &HashMap<String, String>,
  _reported_at_ms: u64,
) -> Result<()> {
  if metrics.is_empty() {
    return Ok(());
  }

  let client = get_db_client(env).await?;
  ensure_alert_tables(&client).await?;

  let rows = client
    .query_typed(
      &format!(
        "SELECT ad.id, ad.user_id, ad.flock_id, ad.pigeon_id, ad.name,
                ad.condition::text AS condition, ad.severity,
                ad.channel::text AS channel, ad.enabled, ad.created_at, ad.updated_at
         FROM alert_definitions ad
         LEFT JOIN pigeons p ON p.id = $1
         WHERE ad.enabled = true
           AND (ad.pigeon_id = $1 OR (ad.flock_id IS NOT NULL AND ad.flock_id = p.flock_id));"
      ),
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert definition lookup failed for pigeon {pigeon_id}: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  for row in &rows {
    let def = AlertDefinition::from(row_to_alert_definition_row(row));

    let AlertCondition::Threshold {
      key,
      comparator,
      value,
    } = &def.condition
    else {
      // DeviceState (and any future variant) isn't ingest-evaluable here --
      // see AlertCondition's doc comment in capsules.
      continue;
    };

    let Some(raw) = metrics.get(key) else {
      continue;
    };
    let Ok(observed) = raw.parse::<f64>() else {
      continue;
    };

    let is_true = comparator.evaluate(observed, *value);

    if let Err(e) = apply_alert_transition(&client, env, &def, pigeon_id, is_true).await {
      console_error!(
        "Alert transition failed for definition {} / pigeon {pigeon_id}: {e}",
        def.id
      );
    }
  }

  Ok(())
}

/// One alert definition's `Ok`/`Firing` state machine for one pigeon
/// (design doc §2.3). Upserts a fresh `alert_state` row on first sight,
/// then applies the transition table described on `capsules::AlertState`'s
/// doc comment. Sends at most one email per transition (fired or cleared);
/// staying `Firing` while still true is intentionally a no-op here --
/// periodic re-notify is an explicitly-deferred, off-by-default extension
/// per the design doc, not implemented in this foundation.
async fn apply_alert_transition(
  client: &Client,
  env: &Env,
  def: &AlertDefinition,
  pigeon_id: &str,
  is_true: bool,
) -> Result<()> {
  let now = OffsetDateTime::now_utc();

  let row = client
    .query_typed_one(
      "INSERT INTO alert_state (alert_definition_id, pigeon_id, status)
       VALUES ($1, $2, 'ok')
       ON CONFLICT (alert_definition_id, pigeon_id) DO UPDATE
         SET alert_definition_id = EXCLUDED.alert_definition_id
       RETURNING status, first_true_at, last_notified_at;",
      &[(&def.id, Type::UUID), (&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!(
        "Alert state upsert failed for definition {} / pigeon {pigeon_id}: {e}",
        def.id
      );
      Error::RustError("Internal Server Error".into())
    })?;

  let status_str: String = row.get("status");
  let status: AlertStatus = status_str.parse().unwrap_or_default();
  let first_true_at: Option<OffsetDateTime> = row.get("first_true_at");

  match (status, is_true) {
    (AlertStatus::Ok, true) => {
      let Some(since) = first_true_at else {
        // Start of a new "true" episode -- record when it began, don't
        // fire until it has stayed true across the debounce window.
        client
          .execute_typed(
            "UPDATE alert_state SET first_true_at = $3 WHERE alert_definition_id = $1 AND pigeon_id = $2;",
            &[(&def.id, Type::UUID), (&pigeon_id, Type::TEXT), (&now, Type::TIMESTAMPTZ)],
          )
          .await
          .map_err(|e| {
            console_error!("Alert state first_true_at write failed: {e}");
            Error::RustError("Internal Server Error".into())
          })?;
        return Ok(());
      };

      if (now - since).whole_seconds() >= ALERT_DEBOUNCE_SECS {
        client
          .execute_typed(
            "UPDATE alert_state SET status = 'firing', last_notified_at = $3
             WHERE alert_definition_id = $1 AND pigeon_id = $2;",
            &[
              (&def.id, Type::UUID),
              (&pigeon_id, Type::TEXT),
              (&now, Type::TIMESTAMPTZ),
            ],
          )
          .await
          .map_err(|e| {
            console_error!("Alert state fire transition failed: {e}");
            Error::RustError("Internal Server Error".into())
          })?;
        send_alert_email(env, client, def, pigeon_id, true).await;
      }
    }
    (AlertStatus::Ok, false) => {
      if first_true_at.is_some() {
        // Blip that never crossed the debounce window -- reset so the next
        // true reading starts a fresh episode.
        client
          .execute_typed(
            "UPDATE alert_state SET first_true_at = NULL WHERE alert_definition_id = $1 AND pigeon_id = $2;",
            &[(&def.id, Type::UUID), (&pigeon_id, Type::TEXT)],
          )
          .await
          .map_err(|e| {
            console_error!("Alert state reset failed: {e}");
            Error::RustError("Internal Server Error".into())
          })?;
      }
    }
    (AlertStatus::Firing, false) => {
      client
        .execute_typed(
          "UPDATE alert_state SET status = 'ok', first_true_at = NULL, last_notified_at = $3
           WHERE alert_definition_id = $1 AND pigeon_id = $2;",
          &[
            (&def.id, Type::UUID),
            (&pigeon_id, Type::TEXT),
            (&now, Type::TIMESTAMPTZ),
          ],
        )
        .await
        .map_err(|e| {
          console_error!("Alert state clear transition failed: {e}");
          Error::RustError("Internal Server Error".into())
        })?;
      send_alert_email(env, client, def, pigeon_id, false).await;
    }
    (AlertStatus::Firing, true) => {
      // Already firing -- no periodic re-notify in this v1 (design doc
      // §2.3: "optional periodic re-notify after a configurable cooldown,
      // off by default" -- not implemented here).
    }
  }

  Ok(())
}

/// Resolves who an alert's notification email should go to (design doc
/// §3.4): the channel's own explicit override if set, otherwise the owning
/// flock's stored `owner_email` -- resolved via this definition's own
/// `flock_id` if flock-scoped, or via its pigeon's `flock_id` if
/// pigeon-scoped. `owner_email` is populated by `lib.rs`'s
/// `require_auth_session`/`helpers/flocks.rs` (`create_user_flock` on
/// create, `backfill_owner_email` opportunistically on `GET /flocks` for
/// flocks that predate this) from the session's own `identity.traits.email`
/// -- a flock created (or listed by its owner) before that landed, or one
/// whose owner has never authenticated since, can still resolve to `None`
/// here, and `send_alert_email` logs that clearly rather than silently
/// dropping the notification.
async fn resolve_alert_recipient(client: &Client, def: &AlertDefinition) -> Option<String> {
  let AlertChannel::Email { to } = &def.channel;
  if let Some(explicit) = to {
    return Some(explicit.clone());
  }

  let result =
    match &def.scope {
      AlertScope::Flock(flock_id) => {
        client
          .query_typed_one(
            "SELECT owner_email FROM flocks WHERE id = $1;",
            &[(flock_id, Type::UUID)],
          )
          .await
      }
      AlertScope::Pigeon(pigeon_id) => client
        .query_typed_one(
          "SELECT f.owner_email FROM flocks f JOIN pigeons p ON p.flock_id = f.id WHERE p.id = $1;",
          &[(pigeon_id, Type::TEXT)],
        )
        .await,
    };

  result.ok().and_then(|row| row.get("owner_email"))
}

async fn send_alert_email(
  env: &Env,
  client: &Client,
  def: &AlertDefinition,
  pigeon_id: &str,
  fired: bool,
) {
  let Some(recipient) = resolve_alert_recipient(client, def).await else {
    console_error!(
      "Alert '{}' ({}): no recipient resolved (owner_email unset and no channel override) -- cannot send {} notification",
      def.name,
      def.id,
      if fired { "fired" } else { "cleared" }
    );
    return;
  };

  let action = if fired { "FIRED" } else { "CLEARED" };
  let subject = format!(
    "[{}] {action}: {}",
    def.severity.as_str().to_uppercase(),
    def.name
  );
  let text = format!(
    "Alert '{}' has {} for pigeon {pigeon_id}.\n\nCondition: {:?}\nSeverity: {}\n",
    def.name,
    if fired { "fired" } else { "cleared" },
    def.condition,
    def.severity.as_str(),
  );

  if let Err(e) = send_via_resend(env, &recipient, &subject, &text).await {
    console_error!("Alert email send failed for definition {}: {e}", def.id);
  }
}

/// `RESEND_API_KEY` Worker secret, if configured -- mirrors
/// `helpers/greptime.rs::greptime_auth_token`'s secret-read shape verbatim
/// (design doc §3.2). Never set via `[vars]`, same rule this codebase
/// already enforces for every credential (`wrangler secret put
/// RESEND_API_KEY --env <env>`).
fn resend_api_key(env: &Env) -> Option<String> {
  env
    .secret("RESEND_API_KEY")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty())
}

#[derive(serde::Serialize)]
struct ResendEmailRequest<'a> {
  from: &'a str,
  to: [&'a str; 1],
  subject: &'a str,
  text: &'a str,
}

/// POSTs one transactional email via Resend's HTTP API
/// (`https://api.resend.com/emails`) -- mirrors
/// `helpers/greptime.rs::post_line_protocol`'s `Fetch`/`RequestInit`/header
/// shape exactly (design doc §3.2). `RESEND_API_KEY` unset (expected until
/// an operator runs `wrangler secret put`) is treated the same way
/// `greptime_auth_token` being absent is treated elsewhere -- logged,
/// never a hard failure, since alert delivery is always best-effort.
async fn send_via_resend(env: &Env, to: &str, subject: &str, text: &str) -> Result<()> {
  let Some(api_key) = resend_api_key(env) else {
    console_error!(
      "RESEND_API_KEY not configured -- cannot send alert email to {to} (subject: {subject})"
    );
    return Ok(());
  };

  let body = ResendEmailRequest {
    from: RESEND_FROM_ADDRESS,
    to: [to],
    subject,
    text,
  };
  let body_json = serde_json::to_string(&body).map_err(|e| {
    console_error!("Failed to serialize Resend request: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  init.body = Some(body_json.into());
  init.headers.set("Content-Type", "application/json")?;
  init
    .headers
    .set("Authorization", &format!("Bearer {api_key}"))?;

  let req = Request::new_with_init("https://api.resend.com/emails", &init)?;
  let resp = Fetch::Request(req).send().await?;

  if resp.status_code() >= 400 {
    console_error!(
      "Resend send to {to} returned HTTP {} (subject: {subject})",
      resp.status_code()
    );
  }

  Ok(())
}
