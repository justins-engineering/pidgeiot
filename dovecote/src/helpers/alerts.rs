use super::timezone::{clock_for, org_timezone};
use crate::helpers::{FlockAccess, PigeonAccess, ResolvedReading, get_db_client, root_url};
use capsules::connection_state::{self, ConnectionState};
use capsules::{
  AlertChannel, AlertCondition, AlertDefinition, AlertDefinitionRow, AlertDefinitionUpdateRequest,
  AlertEmail, AlertObservation, AlertScope, AlertState, AlertStatus, ConnectionStateKind,
  DemoAlert, EmailMessage, JsonString, format_alert_email,
};
use time::OffsetDateTime;
use tokio_postgres::{Client, Row, types::Type};
use uuid::Uuid;
use worker::{
  Env, Error, Fetch, Method, Request, RequestInit, Result, SendEmail, SendEmailBuilder,
  console_error, console_log,
};

/// Column list shared by every `alert_definitions` read/RETURNING statement
/// -- `condition`/`channel` are cast to `::text` rather than read as native
/// JSONB because this workspace's `tokio-postgres` isn't built with the
/// `with-serde_json-1` feature (see `Cargo.toml`). Every other JSONB
/// column in this codebase is only ever written, never read back through
/// `tokio-postgres` directly, so this cast is the read-side mirror of the
/// `$N::jsonb` write pattern those columns already use.
const ALERT_DEFINITION_COLUMNS: &str = "id, user_id, flock_id, pigeon_id, name, \
  condition::text AS condition, severity, channel::text AS channel, notes, enabled, \
  created_at, updated_at";

/// Fixed debounce window before a continuously-true condition transitions
/// `Ok -> Firing`. Scaling this per-pigeon off `telemetry_interval` the
/// way `connection_state::classify` (`capsules::connection_state`) already
/// does would be reasonable; a single fixed window is a deliberate
/// simplification, not an oversight.
const ALERT_DEBOUNCE_SECS: i64 = 60;

/// `From:` fallback for platform mail -- shares the platform's one
/// verified useSend sending domain with Kratos's courier setup, but never
/// the credential. The Worker secret is still NAMED `RESEND_API_KEY` for
/// historical reasons but holds a useSend API key -- useSend speaks the
/// Resend-shaped payload, so sends 401 against api.resend.com if this ever
/// gets swapped for a real Resend key (see `post_via_usesend` below).
const DEFAULT_FROM_ADDRESS: &str = "alerts@noreply.pidgeiot.com";

/// Cloudflare Email Service binding (`[[send_email]]`, wrangler.toml).
/// Its presence is what selects that transport over useSend, so it is
/// declared only in the environments whose sending domain is onboarded.
const EMAIL_BINDING: &str = "EMAIL";

/// Idempotently ensures the `alert_definitions`/`alert_state` tables (+
/// indexes) exist -- mirrors `ensure_telemetry_history_table`/
/// `ensure_flock_firmware_table`'s rationale: no environment has a
/// separate migration runner against its Hyperdrive Postgres.
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
      ALTER TABLE alert_definitions ADD COLUMN IF NOT EXISTS notes TEXT;
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
    notes: row.get("notes"),
    enabled: row.get("enabled"),
    created_at: row.get("created_at"),
    updated_at: row.get("updated_at"),
  }
}

/// Stored form of an alert's notes. The routes have already refused
/// anything past `MAX_ALERT_NOTES_BYTES`; dropping the notes rather than
/// storing an oversized value is the safe reading of a request that got
/// here another way.
fn normalized_notes(notes: Option<&str>) -> Option<String> {
  capsules::normalize_alert_notes(notes).unwrap_or_default()
}

/// `capsules::AlertState` has no `*Row` variant (see its doc comment --
/// Postgres hands back a native `OffsetDateTime` for every `TIMESTAMPTZ`
/// column here, same as `Flock`/`FirmwareImage`), so this reads straight
/// into the public API shape, same as `list_flock_firmware`'s row mapping.
fn row_to_alert_state(row: &Row) -> AlertState {
  let status_str: String = row.get("status");
  AlertState {
    alert_definition_id: row.get("alert_definition_id"),
    pigeon_id: row.get("pigeon_id"),
    status: status_str.parse().unwrap_or_default(),
    first_true_at: row.get("first_true_at"),
    last_notified_at: row.get("last_notified_at"),
  }
}

/// Proof that `is_alert_owner` already confirmed the requesting user owns
/// this alert definition (`alert_definitions.user_id`) -- same
/// "caller must have already checked" guard as `PigeonAccess`/`FlockAccess`,
/// applied to alert ownership.
pub struct AlertAccess {
  alert_id: Uuid,
  scope: AlertScope,
}

impl AlertAccess {
  pub fn alert_id(&self) -> Uuid {
    self.alert_id
  }

  /// What the definition is scoped to, read by the same query that proved
  /// ownership -- the update route needs it to resolve which addresses
  /// this alert may be aimed at.
  pub fn scope(&self) -> &AlertScope {
    &self.scope
  }
}

/// Every address this account may aim an alert at: its own verified Kratos
/// addresses, the owning flock's stored `owner_email`, and the addresses of
/// the members of the organization that owns that flock. Signup is open, so
/// an unrestricted recipient would make alert mail an arbitrary-content
/// relay; a recipient always has to be one the platform already ties to
/// this account. Case-folded, since an address is compared, not displayed.
pub async fn allowed_alert_recipients(
  client: &Client,
  scope: &AlertScope,
  verified_emails: &[String],
) -> Result<Vec<String>> {
  ensure_alert_tables(client).await?;

  // string_agg rather than an array: every address here has already been
  // shape-checked against `is_plausible_email`, which refuses commas.
  const MEMBER_EMAILS: &str = "(SELECT string_agg(DISTINCT m.email, ',')
     FROM organization_members m
     WHERE m.org_id = f.org_id AND m.email IS NOT NULL) AS member_emails";

  let row = match scope {
    AlertScope::Flock(flock_id) => {
      client
        .query_typed_opt(
          &format!("SELECT f.owner_email, {MEMBER_EMAILS} FROM flocks f WHERE f.id = $1;"),
          &[(flock_id, Type::UUID)],
        )
        .await
    }
    AlertScope::Pigeon(pigeon_id) => {
      client
        .query_typed_opt(
          &format!(
            "SELECT f.owner_email, {MEMBER_EMAILS}
             FROM flocks f JOIN pigeons p ON p.flock_id = f.id
             WHERE p.id = $1;"
          ),
          &[(pigeon_id, Type::TEXT)],
        )
        .await
    }
  }
  .map_err(|e| {
    console_error!("Alert recipient allowlist lookup failed: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let owner_email: Option<String> = row.as_ref().and_then(|row| row.get("owner_email"));
  let member_emails: Option<String> = row.as_ref().and_then(|row| row.get("member_emails"));

  let mut allowed: Vec<String> = verified_emails
    .iter()
    .map(|e| e.trim().to_lowercase())
    .collect();
  allowed.extend(owner_email.iter().map(|e| e.trim().to_lowercase()));
  allowed.extend(
    member_emails
      .iter()
      .flat_map(|list| list.split(','))
      .map(|e| e.trim().to_lowercase()),
  );
  allowed.retain(|e| !e.is_empty());
  allowed.sort_unstable();
  allowed.dedup();

  Ok(allowed)
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
    .query_typed_opt(
      "SELECT flock_id, pigeon_id FROM alert_definitions WHERE id = $1 AND user_id = $2;",
      &[(&alert_uuid, Type::UUID), (&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert ownership check query error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(row.map(|row| {
    let flock_id: Option<Uuid> = row.get("flock_id");
    let pigeon_id: Option<String> = row.get("pigeon_id");
    // The table's own CHECK constraint guarantees exactly one is set; the
    // empty-pigeon fallback mirrors `AlertDefinitionRow`'s conversion.
    let scope = match (pigeon_id, flock_id) {
      (Some(id), _) => AlertScope::Pigeon(id),
      (None, Some(id)) => AlertScope::Flock(id),
      (None, None) => AlertScope::Pigeon(String::new()),
    };
    AlertAccess {
      alert_id: alert_uuid,
      scope,
    }
  }))
}

/// Creates a pigeon-scoped alert definition. Takes a `PigeonAccess` proof
/// (not a bare `pigeon_id`) -- only constructible via `check_pigeon_authz`,
/// which is what actually confirmed this user can act on this pigeon, same
/// guard `query_telemetry_history_for_pigeon` already requires.
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
  let notes = normalized_notes(req.notes.as_deref());

  let row = client
    .query_typed_one(
      &format!(
        "INSERT INTO alert_definitions
           (user_id, pigeon_id, name, condition, severity, channel, notes)
         VALUES ($1, $2, $3, $4::jsonb, $5, $6::jsonb, $7)
         RETURNING {ALERT_DEFINITION_COLUMNS};"
      ),
      &[
        (&user_uuid, Type::UUID),
        (&pigeon_id, Type::TEXT),
        (&req.name, Type::TEXT),
        (&condition_json, Type::TEXT),
        (&severity_str, Type::TEXT),
        (&channel_json, Type::TEXT),
        (&notes, Type::TEXT),
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
  let notes = normalized_notes(req.notes.as_deref());

  let row = client
    .query_typed_one(
      &format!(
        "INSERT INTO alert_definitions
           (user_id, flock_id, name, condition, severity, channel, notes)
         VALUES ($1, $2, $3, $4::jsonb, $5, $6::jsonb, $7)
         RETURNING {ALERT_DEFINITION_COLUMNS};"
      ),
      &[
        (&user_uuid, Type::UUID),
        (&flock_uuid, Type::UUID),
        (&req.name, Type::TEXT),
        (&condition_json, Type::TEXT),
        (&severity_str, Type::TEXT),
        (&channel_json, Type::TEXT),
        (&notes, Type::TEXT),
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
/// not inlined here; the dashboard's flock-level alerts tab is where a
/// flock-scoped alert is expected to show up instead.
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

/// Backs `GET /pigeons/:pigeon_id/alerts/state` -- the current fired/ok
/// status of every alert scoped directly to this pigeon (design doc §2.3,
/// gap G3). Same scope restriction as `list_pigeon_alerts`: flock-scoped
/// alerts that happen to cover this pigeon are not included here either,
/// so a caller wanting both reads `list_pigeon_alerts`/`list_flock_alerts`
/// (or their `/state` counterparts) side by side, same as the existing
/// pair. A definition the evaluator has never run against has no
/// `alert_state` row yet and so is simply absent from this list -- callers
/// counting firing alerts don't need a special case for "never evaluated",
/// since an absent row can't be `Firing`.
pub async fn list_pigeon_alert_state(
  client: &Client,
  access: &PigeonAccess,
) -> Result<Vec<AlertState>> {
  ensure_alert_tables(client).await?;

  let pigeon_id = access.pigeon_id();
  let rows = client
    .query_typed(
      "SELECT s.alert_definition_id, s.pigeon_id, s.status, s.first_true_at, s.last_notified_at
       FROM alert_state s
       JOIN alert_definitions d ON d.id = s.alert_definition_id
       WHERE d.pigeon_id = $1;",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert state list error (pigeon scope): {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.iter().map(row_to_alert_state).collect())
}

/// Backs `GET /flocks/:flock_id/alerts/state`. A flock-scoped alert can
/// carry several rows here -- one per pigeon in the flock the evaluator has
/// run it against, since it fires/clears independently per pigeon (see
/// `capsules::AlertState`'s doc comment) -- so this is not 1:1 with
/// `list_flock_alerts`'s definition count. Counting `Firing` rows is what a
/// fleet/flock "open alerts" KPI wants, not counting definitions.
pub async fn list_flock_alert_state(
  client: &Client,
  access: &FlockAccess,
) -> Result<Vec<AlertState>> {
  ensure_alert_tables(client).await?;

  let flock_uuid = Uuid::parse_str(access.flock_id())
    .map_err(|e| Error::RustError(format!("Invalid flock_id format: {e}")))?;
  let rows = client
    .query_typed(
      "SELECT s.alert_definition_id, s.pigeon_id, s.status, s.first_true_at, s.last_notified_at
       FROM alert_state s
       JOIN alert_definitions d ON d.id = s.alert_definition_id
       WHERE d.flock_id = $1;",
      &[(&flock_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Alert state list error (flock scope): {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.iter().map(row_to_alert_state).collect())
}

/// Backs the public, unauthenticated `GET /demo/pigeons/:pigeon_id/alerts`.
///
/// The column list is deliberately not `ALERT_DEFINITION_COLUMNS`: this
/// response goes to anyone who asks, so `user_id` (an account UUID) and
/// `channel` (an `AlertChannel::Email` holding a real address) are never
/// read out of the database on this path at all. Nothing between here and
/// the response body holds a value it has to remember not to serialize.
///
/// Pigeon-scoped definitions only, though a flock-scoped alert can govern
/// this pigeon too: a flock alert is shared configuration, so publishing it
/// here would describe a rule covering pigeons that are not on the demo
/// allowlist. Disabled definitions are excluded for a different reason —
/// the demo's whole claim is that the platform is really enforcing the line
/// on the chart, and a disabled alert is a threshold nothing checks.
///
/// The `LEFT JOIN` is what makes an alert that has never been evaluated
/// report `Ok` rather than vanishing from the list.
pub async fn list_demo_pigeon_alerts(
  client: &Client,
  access: &PigeonAccess,
) -> Result<Vec<DemoAlert>> {
  ensure_alert_tables(client).await?;

  let pigeon_id = access.pigeon_id();
  let rows = client
    .query_typed(
      "SELECT d.name, d.condition::text AS condition, d.severity,
              COALESCE(s.status, 'ok') AS status
       FROM alert_definitions d
       LEFT JOIN alert_state s
         ON s.alert_definition_id = d.id AND s.pigeon_id = $1
       WHERE d.pigeon_id = $1 AND d.enabled = true
       ORDER BY d.created_at DESC;",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Demo alert list error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .iter()
      .map(|row| {
        let condition_json: String = row.get("condition");
        let severity: String = row.get("severity");
        let status: String = row.get("status");

        DemoAlert::project(
          row.get("name"),
          severity.parse().unwrap_or_default(),
          status.parse().unwrap_or_default(),
          &serde_json::from_str(&condition_json).unwrap_or_default(),
        )
      })
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
  // Notes need the submitted/omitted distinction COALESCE cannot express:
  // an omitted `notes` keeps what is stored, an empty one clears it.
  let notes_submitted = req.notes.is_some();
  let notes = normalized_notes(req.notes.as_deref());

  let row = client
    .query_typed_one(
      &format!(
        "UPDATE alert_definitions SET
           name = COALESCE($2, name),
           condition = COALESCE($3::jsonb, condition),
           severity = COALESCE($4, severity),
           channel = COALESCE($5::jsonb, channel),
           enabled = COALESCE($6, enabled),
           notes = CASE WHEN $7 THEN $8 ELSE notes END,
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
        (&notes_submitted, Type::BOOL),
        (&notes, Type::TEXT),
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

/// Evaluation hook -- called alongside `write_telemetry_default_batch` at
/// each of its three call sites (`queue.rs::store_and_alert`,
/// `objects/pigeons.rs::handle_ws_telemetry`/`report_telemetry_device`),
/// NOT only from `queue.rs`: `queue.rs` alone misses dev entirely (no
/// queue bound) and always misses WS-telemetry. Best-effort: every
/// failure is logged, never propagated to fail the caller's own
/// (already-succeeded) primary write.
///
/// Resolves every enabled alert definition scoped either directly to this
/// pigeon or to the flock it belongs to (one query, via a LEFT JOIN
/// against `pigeons` rather than a second round trip to resolve
/// `flock_id` first). `Threshold` and `RateOfChange` conditions are
/// evaluated here -- see `AlertCondition`'s doc comment in `capsules` for
/// why `DeviceState`/`MissingReport` are no-ops in this hook (evaluated
/// instead by `evaluate_scheduled_alerts` below, the Cron-Trigger-driven
/// sweep). `previous` is each reported key's prior value + timestamp,
/// captured by the caller (`objects/pigeons.rs::read_previous_telemetry`)
/// before its own UPSERT overwrote it -- the only input `RateOfChange`
/// needs that `Threshold` doesn't; see `PreviousTelemetryValue`'s doc
/// comment for why that capture has to happen at the call site, not here.
/// The batched form, and the one that holds the evaluation loop -- a
/// single report is a batch of one.
///
/// An alert has to see what a batch actually contains. A device buffering
/// ten minutes of readings and sending them together must trip the same
/// alerts, at the same points, as the device next to it sending each
/// reading as it takes it; evaluating only the batch's final values would
/// make a spike that rose and fell inside one batch invisible, and would
/// hand `RateOfChange` a single jump where the device recorded a gradual
/// climb. So this walks the batch chronologically, evaluating every
/// definition against every reading, with each reading's own timestamp
/// driving the debounce window.
///
/// What it does NOT do is call the transition machinery once per reading.
/// The definitions are fetched once for the whole batch, and a transition
/// is applied only where the condition's truth CHANGES, plus once at the
/// end of the batch. That is enough to reproduce the sequential outcome:
/// the first evaluation of a true run opens the debounce window and the
/// last one closes it, which is exactly the pair of calls a stream of
/// separate reports would have used to fire. It costs one round trip for
/// a batch that holds steady rather than sixty-four, which is the whole
/// reason the batch is worth sending.
///
/// The one visible difference from sequential arrival: a firing alert
/// records the batch's last matching reading as `last_notified_at` rather
/// than the earliest reading that satisfied the debounce. Nothing reads
/// that column to decide whether to notify (there is no re-notify), so it
/// costs an approximate timestamp and saves the round trips.
pub async fn check_telemetry_alerts_batch(
  env: &Env,
  pigeon_id: &str,
  readings: &[ResolvedReading],
) -> Result<()> {
  if readings.iter().all(|reading| reading.metrics.is_empty()) {
    return Ok(());
  }

  let client = get_db_client(env).await?;
  ensure_alert_tables(&client).await?;

  let rows = client
    .query_typed(
      &format!(
        "SELECT ad.id, ad.user_id, ad.flock_id, ad.pigeon_id, ad.name,
                ad.condition::text AS condition, ad.severity,
                ad.channel::text AS channel, ad.notes, ad.enabled, ad.created_at,
                ad.updated_at
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

    // Only the readings this definition can actually be decided against:
    // a reading missing the key, or carrying a non-numeric value for it,
    // says nothing about the condition either way and must not be read as
    // a recovery.
    let evaluated: Vec<(i64, bool, AlertObservation)> = readings
      .iter()
      .filter_map(|reading| {
        evaluate_ingest_condition(&def.condition, reading)
          .map(|(is_true, observation)| (reading.at_secs, is_true, observation))
      })
      .collect();

    for (at_secs, is_true, observation) in transitions_to_apply(&evaluated) {
      let at = OffsetDateTime::from_unix_timestamp(at_secs).unwrap_or(OffsetDateTime::now_utc());
      if let Err(e) =
        apply_alert_transition(&client, env, &def, pigeon_id, is_true, &observation, at).await
      {
        console_error!(
          "Alert transition failed for definition {} / pigeon {pigeon_id}: {e}",
          def.id
        );
      }
    }
  }

  Ok(())
}

/// Which of a batch's decided readings actually need a trip through the
/// transition machinery: every reading where the condition's truth changes
/// from the one before it, plus the batch's last decided reading.
///
/// Those two are what the state machine needs and all it needs. Opening a
/// true run records when it began; closing it is what compares that
/// against the debounce window and fires. Everything between the two would
/// re-read and rewrite the same row to reach the same conclusion, which
/// for a device buffering sixty-four readings is sixty-two round trips
/// spent to change nothing. Whatever rides along with each verdict (the
/// observation the notification will quote) is carried through untouched.
fn transitions_to_apply<T: Clone>(evaluated: &[(i64, bool, T)]) -> Vec<(i64, bool, T)> {
  let mut applied: Option<bool> = None;
  let mut out = Vec::new();

  for (index, (at_secs, is_true, seen)) in evaluated.iter().enumerate() {
    let is_last = index + 1 == evaluated.len();
    if applied == Some(*is_true) && !is_last {
      continue;
    }
    applied = Some(*is_true);
    out.push((*at_secs, *is_true, seen.clone()));
  }

  out
}

/// One reading against one condition: the verdict plus what was observed
/// when the reading decides it, `None` when it says nothing -- the key is
/// absent, its value is not a number, or the condition is one of the
/// absence-of-signal kinds that only the scheduled sweep can decide (see
/// `AlertCondition`'s doc comment in `capsules`, and
/// `evaluate_scheduled_alerts` below). The observation is what lets the
/// notification quote the value next to the threshold it crossed.
fn evaluate_ingest_condition(
  condition: &AlertCondition,
  reading: &ResolvedReading,
) -> Option<(bool, AlertObservation)> {
  match condition {
    AlertCondition::Threshold {
      key,
      comparator,
      value,
    } => {
      let observed = reading.metrics.get(key)?.parse::<f64>().ok()?;
      Some((
        comparator.evaluate(observed, *value),
        AlertObservation::Value { observed },
      ))
    }
    AlertCondition::RateOfChange {
      key,
      max_delta,
      window_secs,
    } => {
      let observed = reading.metrics.get(key)?.parse::<f64>().ok()?;
      // No previous entry for this key -- either this pigeon's first-ever
      // report of it, or the previous value wasn't numeric. Either way,
      // nothing to diff against yet, so this can never fire on a first
      // reading (capsules::AlertCondition::RateOfChange's own doc
      // comment). Inside a batch the previous entry is the reading before
      // this one, so a climb is measured step by step.
      let prev = reading.previous.as_ref()?.get(key)?;
      let prev_value = prev.value.parse::<f64>().ok()?;

      if let Some(window) = window_secs {
        let gap_secs = reading.at_secs - prev.reported_at;
        if gap_secs > *window {
          // The two samples are too far apart in time to call their
          // difference a "rate" of anything (e.g. resuming after a long
          // silence at a very different reading is not a spike) -- say
          // nothing rather than fire.
          return None;
        }
      }

      Some((
        (observed - prev_value).abs() > *max_delta,
        AlertObservation::Change {
          previous: prev_value,
          observed,
        },
      ))
    }
    // DeviceState/MissingReport (and any future absence-of-signal
    // variant) aren't ingest-evaluable here.
    AlertCondition::DeviceState { .. } | AlertCondition::MissingReport { .. } => None,
  }
}

/// Cron-Trigger-driven scheduled evaluator -- the counterpart to
/// `check_telemetry_alerts` above for the two condition types that can't
/// be decided at ingest time: an ingest event arriving is itself proof
/// the pigeon is online, so "went offline/stale" (`DeviceState`) and
/// "nothing has arrived in N seconds" (`MissingReport`) both have to be
/// polled on a timer instead. Wired up via `wrangler.toml`'s `[triggers]
/// crons` and `src/scheduled.rs`'s `#[event(scheduled)]` handler, which
/// just calls this and logs whatever it returns -- best-effort/logged
/// throughout; a failure here must never panic the scheduled invocation.
///
/// Deliberately does NOT fan out to every matching pigeon's own Durable
/// Object -- same reasoning against per-DO fan-out for cross-pigeon
/// queries as `docs/design/tenancy-isolation.md`. "Last seen" here is
/// resolved entirely from Postgres, via `resolve_pigeon_last_seen` below:
/// `pigeon_shadow.updated_at` (filtered through
/// `connection_state::has_never_reported`, same rule `fancier`'s
/// `PigeonView` already applies) merged with the newest
/// `pigeon_telemetry_history` row, through the same
/// `connection_state::classify`/`latest_of` this crate shares with
/// `fancier`'s connection badge (`capsules::connection_state`).
///
/// Known gap, documented rather than silently accepted: a pigeon with a
/// user-configured `telemetry_endpoint` never gets a row in
/// `pigeon_telemetry_history` -- its reports go to that endpoint's target
/// instead of Postgres/Greptime history, so this sweep can only see its
/// shadow signal. Good enough for a v1 scheduled evaluator; a future
/// iteration could also consult Greptime the way
/// `query_greptime_history_for_pigeons` already does for the dashboard's
/// own history routes.
pub async fn evaluate_scheduled_alerts(env: &Env) -> Result<()> {
  let client = get_db_client(env).await?;
  ensure_alert_tables(&client).await?;

  // The jsonb `?` "does this top-level key exist" operator matches
  // AlertCondition's externally-tagged serde encoding exactly (a
  // `DeviceState` value serializes to `{"DeviceState": {...}}`) -- same
  // idea as the `ad.pigeon_id = $1 OR ...` scoping check
  // check_telemetry_alerts already does, just expressed against the JSON
  // shape instead of a plain column.
  let rows = client
    .query_typed(
      &format!(
        "SELECT {ALERT_DEFINITION_COLUMNS} FROM alert_definitions
         WHERE enabled = true
           AND (condition ? 'DeviceState' OR condition ? 'MissingReport');"
      ),
      &[],
    )
    .await
    .map_err(|e| {
      console_error!("Scheduled alert eval: definition lookup failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let now = OffsetDateTime::now_utc();

  for row in &rows {
    let def = AlertDefinition::from(row_to_alert_definition_row(row));

    let pigeon_ids = match resolve_scope_pigeon_ids(&client, &def.scope).await {
      Ok(ids) => ids,
      Err(e) => {
        console_error!(
          "Scheduled alert eval: scope resolution failed for definition {}: {e}",
          def.id
        );
        continue;
      }
    };

    for pigeon_id in pigeon_ids {
      let seen = match resolve_pigeon_last_seen(&client, &pigeon_id).await {
        Ok(Some(seen)) => seen,
        // No pigeon_shadow row -- e.g. the pigeon was deleted between the
        // scope resolution above and this lookup. Nothing to evaluate.
        Ok(None) => continue,
        Err(e) => {
          console_error!(
            "Scheduled alert eval: last-seen lookup failed for definition {} / pigeon {pigeon_id}: {e}",
            def.id
          );
          continue;
        }
      };

      let is_true = match &def.condition {
        AlertCondition::DeviceState {
          state,
          min_duration_secs,
        } => {
          let classified = connection_state::classify(seen.last_seen, seen.interval_secs, now);
          let target = match state {
            ConnectionStateKind::Offline => ConnectionState::Offline,
            ConnectionStateKind::Stale => ConnectionState::Stale,
          };
          let mut matched = classified == target;
          if matched {
            if let Some(min_secs) = min_duration_secs {
              // How long the pigeon has been silent doubles as "how long
              // it's been in this state" -- it entered Offline/Stale the
              // moment it stopped reporting, so the age of its last-seen
              // signal already is that duration.
              let age_secs = seen
                .last_seen
                .map(|t| (now - t).whole_seconds())
                .unwrap_or(i64::MAX);
              matched = age_secs >= *min_secs;
            }
          }
          matched
        }
        AlertCondition::MissingReport { max_silence_secs } => match seen.last_seen {
          None => true,
          Some(t) => (now - t).whole_seconds() >= *max_silence_secs,
        },
        // The query above only ever selects DeviceState/MissingReport
        // definitions -- Threshold/RateOfChange never reach this loop, but
        // the match stays exhaustive rather than reaching for a wildcard
        // arm that would silently swallow a future variant too.
        AlertCondition::Threshold { .. } | AlertCondition::RateOfChange { .. } => continue,
      };

      let observation = AlertObservation::Silence {
        last_seen: seen.last_seen,
      };
      if let Err(e) =
        apply_alert_transition(&client, env, &def, &pigeon_id, is_true, &observation, now).await
      {
        console_error!(
          "Scheduled alert eval: transition failed for definition {} / pigeon {pigeon_id}: {e}",
          def.id
        );
      }
    }
  }

  Ok(())
}

/// Every pigeon_id a `DeviceState`/`MissingReport` definition's scope
/// resolves to -- `Pigeon` is trivially itself; `Flock` needs a lookup
/// since a flock-scoped alert fires/clears independently per pigeon
/// currently in it (`capsules::AlertScope`'s own doc comment). No
/// ownership re-check here, unlike `helpers::telemetry::get_flock_pigeon_ids`
/// (which gates a *user's* dashboard request) -- this runs from the
/// scheduled sweep, not on behalf of any one user, and the definition was
/// already created through an owner-gated route
/// (`create_flock_alert`/`create_pigeon_alert`, both take an
/// already-checked `FlockAccess`/`PigeonAccess`), so re-deriving ownership
/// here would just re-answer a question already settled at creation time.
async fn resolve_scope_pigeon_ids(client: &Client, scope: &AlertScope) -> Result<Vec<String>> {
  match scope {
    AlertScope::Pigeon(pigeon_id) => Ok(vec![pigeon_id.clone()]),
    AlertScope::Flock(flock_id) => {
      let rows = client
        .query_typed(
          "SELECT id FROM pigeons WHERE flock_id = $1;",
          &[(flock_id, Type::UUID)],
        )
        .await
        .map_err(|e| {
          console_error!(
            "Scheduled alert eval: flock pigeon lookup failed for flock {flock_id}: {e}"
          );
          Error::RustError("Internal Server Error".into())
        })?;
      Ok(rows.into_iter().map(|row| row.get("id")).collect())
    }
  }
}

/// One pigeon's merged "last seen" signal + its own reporting cadence, as
/// resolved from Postgres for `evaluate_scheduled_alerts` -- see that
/// function's doc comment for the merge rule and its documented gap
/// (telemetry-endpoint-forwarding pigeons).
struct PigeonLastSeen {
  last_seen: Option<OffsetDateTime>,
  interval_secs: Option<i64>,
}

/// Resolves one pigeon's shadow + telemetry-history state in a single
/// round trip: `pigeon_shadow` for `current_version`/`current_config`
/// (feeding `has_never_reported`/`telemetry_interval_secs`) and
/// `updated_at`, LEFT JOINed against a `MAX(reported_at)` aggregate over
/// `pigeon_telemetry_history` for this pigeon (an aggregate with no
/// `GROUP BY` always returns exactly one row, even when zero telemetry
/// rows match, so `ON true` always finds a match -- this can only return
/// `Ok(None)` when `pigeon_shadow` itself has no row, i.e. an already
/// (or concurrently) deleted pigeon). Returns `Ok(Some(_))` with
/// `last_seen: None` for a pigeon that has genuinely never reported
/// anything, matching `classify`'s own `Unknown` handling.
async fn resolve_pigeon_last_seen(
  client: &Client,
  pigeon_id: &str,
) -> Result<Option<PigeonLastSeen>> {
  let row = client
    .query_typed_opt(
      "SELECT s.current_version, s.current_config::text AS current_config,
              s.updated_at AS shadow_updated_at, t.last_at
       FROM pigeon_shadow s
       LEFT JOIN (
         SELECT MAX(reported_at) AS last_at
         FROM pigeon_telemetry_history
         WHERE pigeon_id = $1
       ) t ON true
       WHERE s.id = $1;",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Scheduled alert eval: last-seen lookup failed for pigeon {pigeon_id}: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(row) = row else {
    return Ok(None);
  };

  let current_version: i32 = row.get("current_version");
  let current_config_raw: String = row.get("current_config");
  let shadow_updated_at: i64 = row.get("shadow_updated_at");
  let telemetry_last_at: Option<OffsetDateTime> = row.get("last_at");

  let config = JsonString::new(current_config_raw).ok();

  let shadow_last_seen = config
    .as_ref()
    .filter(|c| !connection_state::has_never_reported(current_version, c))
    .and_then(|_| OffsetDateTime::from_unix_timestamp(shadow_updated_at).ok());

  let interval_secs = config
    .as_ref()
    .and_then(connection_state::telemetry_interval_secs);

  Ok(Some(PigeonLastSeen {
    last_seen: connection_state::latest_of([shadow_last_seen, telemetry_last_at]),
    interval_secs,
  }))
}

/// One alert definition's `Ok`/`Firing` state machine for one pigeon.
/// Upserts a fresh `alert_state` row on first sight, then applies the
/// transition table described on `capsules::AlertState`'s doc comment.
/// Sends at most one email per transition (fired or cleared); staying
/// `Firing` while still true is intentionally a no-op -- periodic
/// re-notify is a deferred, off-by-default extension, not implemented
/// here.
async fn apply_alert_transition(
  client: &Client,
  env: &Env,
  def: &AlertDefinition,
  pigeon_id: &str,
  is_true: bool,
  observation: &AlertObservation,
  now: OffsetDateTime,
) -> Result<()> {
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
        send_alert_email(env, client, def, pigeon_id, true, observation, now).await;
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
      send_alert_email(env, client, def, pigeon_id, false, observation, now).await;
    }
    (AlertStatus::Firing, true) => {
      // Already firing -- no periodic re-notify implemented (would be an
      // optional cooldown-gated re-send, off by default).
    }
  }

  Ok(())
}

/// What one notification needs from Postgres, fetched in a single round
/// trip: who it goes to, and the names it should call the pigeon and its
/// flock by.
struct AlertMailContext {
  recipients: Vec<String>,
  pigeon_name: Option<String>,
  flock_id: Option<Uuid>,
  flock_name: Option<String>,
  /// The owning organization's zone, when the flock belongs to one. A
  /// personal flock has no organization and therefore no zone, so its
  /// notifications stay in UTC.
  timezone: Option<String>,
}

/// Resolves who an alert's notification email should go to: the channel's
/// own recipient list if it names anyone, otherwise the owning flock's
/// stored `owner_email` -- resolved via this definition's own `flock_id` if
/// flock-scoped, or via its pigeon's `flock_id` if pigeon-scoped.
/// `owner_email` is populated by `lib.rs`'s
/// `require_auth_session`/`helpers/flocks.rs` (`create_user_flock` on
/// create, `backfill_owner_email` opportunistically on `GET /flocks`) from
/// the session's own `identity.traits.email` -- a flock whose owner has
/// never authenticated since can still resolve to `None` here, and
/// `send_alert_email` logs that clearly rather than silently dropping the
/// notification. The same row carries the pigeon and flock names.
async fn resolve_alert_context(
  client: &Client,
  def: &AlertDefinition,
  pigeon_id: &str,
) -> AlertMailContext {
  let AlertChannel::Email { to } = &def.channel;

  const COLUMNS: &str = "f.owner_email, f.id AS flock_id, f.name AS flock_name,
     p.name AS pigeon_name, o.timezone AS org_timezone,
     (SELECT string_agg(DISTINCT m.email, ',')
        FROM organization_members m
        WHERE m.org_id = f.org_id AND m.email IS NOT NULL) AS member_emails";

  let result = match &def.scope {
    AlertScope::Flock(flock_id) => {
      client
        .query_typed_opt(
          &format!(
            "SELECT {COLUMNS}
             FROM flocks f
             LEFT JOIN pigeons p ON p.id = $2 AND p.flock_id = f.id
             LEFT JOIN organizations o ON o.id = f.org_id
             WHERE f.id = $1;"
          ),
          &[(flock_id, Type::UUID), (&pigeon_id, Type::TEXT)],
        )
        .await
    }
    AlertScope::Pigeon(_) => {
      client
        .query_typed_opt(
          &format!(
            "SELECT {COLUMNS}
             FROM flocks f
             JOIN pigeons p ON p.flock_id = f.id
             LEFT JOIN organizations o ON o.id = f.org_id
             WHERE p.id = $1;"
          ),
          &[(&pigeon_id, Type::TEXT)],
        )
        .await
    }
  };

  let row = result.ok().flatten();
  let owner_email: Option<String> = row.as_ref().and_then(|row| row.get("owner_email"));
  let member_emails: Option<String> = row.as_ref().and_then(|row| row.get("member_emails"));
  let mut context = AlertMailContext {
    recipients: owner_email.iter().cloned().collect(),
    pigeon_name: row.as_ref().and_then(|row| row.get("pigeon_name")),
    flock_id: row.as_ref().and_then(|row| row.get("flock_id")),
    flock_name: row.as_ref().and_then(|row| row.get("flock_name")),
    timezone: row.as_ref().and_then(|row| row.get("org_timezone")),
  };

  // Defense-in-depth: the create/update routes already reject a recipient
  // the account has no claim to, but definitions that predate that
  // validation (or rows written outside the API) could still carry an
  // arbitrary address. At send time a recipient is honored only if it is
  // still the flock's owner or a member of the organization that owns it
  // -- anything else is dropped with a log line, so alert delivery can
  // never be aimed at an address the platform hasn't tied to this account.
  if !to.is_empty() {
    let mut deliverable: Vec<String> = Vec::with_capacity(to.len());
    let mut refused = 0usize;
    for address in to {
      let address = address.trim();
      let known = owner_email
        .iter()
        .map(String::as_str)
        .chain(member_emails.iter().flat_map(|list| list.split(',')))
        .any(|known| known.trim().eq_ignore_ascii_case(address));
      if known {
        deliverable.push(address.to_string());
      } else {
        refused += 1;
      }
    }
    if refused > 0 {
      console_error!(
        "Alert '{}' ({}): dropping {refused} recipient(s) no longer tied to this account",
        def.name,
        def.id
      );
    }
    if !deliverable.is_empty() {
      context.recipients = deliverable;
    }
  }

  context
}

async fn send_alert_email(
  env: &Env,
  client: &Client,
  def: &AlertDefinition,
  pigeon_id: &str,
  fired: bool,
  observation: &AlertObservation,
  at: OffsetDateTime,
) {
  let context = resolve_alert_context(client, def, pigeon_id).await;
  if context.recipients.is_empty() {
    console_error!(
      "Alert '{}' ({}): no recipient resolved (owner_email unset and no channel recipients) -- cannot send {} notification",
      def.name,
      def.id,
      if fired { "fired" } else { "cleared" }
    );
    return;
  }

  // A pigeon-scoped alert is edited from the pigeon's own page, a
  // flock-scoped one from the flock's pigeon list; both sections carry a
  // stable id the dashboard can be deep-linked to.
  let root = root_url(env);
  let (pigeon_url, manage_url) = match context.flock_id {
    Some(flock_id) => {
      let pigeon_url = format!("{root}/flocks/{flock_id}/pigeons/{pigeon_id}");
      let manage_url = match &def.scope {
        AlertScope::Flock(_) => format!("{root}/flocks/{flock_id}/pigeons#flockAlerts"),
        AlertScope::Pigeon(_) => format!("{pigeon_url}#pigeonAlerts"),
      };
      (pigeon_url, manage_url)
    }
    None => (format!("{root}/flocks"), format!("{root}/flocks")),
  };

  // An org-owned flock is read by a team that shares one wall clock; a
  // personal flock has no organization to ask, so it stays in UTC.
  let zone = context.timezone.as_deref().and_then(org_timezone);
  if zone.is_none()
    && context
      .timezone
      .as_deref()
      .is_some_and(|name| name != capsules::DEFAULT_TIMEZONE)
  {
    console_error!(
      "Alert '{}' ({}): the organization's timezone is not one the database knows; stamping in UTC instead",
      def.name,
      def.id
    );
  }

  let message = format_alert_email(
    &AlertEmail {
      definition: def,
      fired,
      pigeon_id,
      pigeon_name: context.pigeon_name.as_deref(),
      flock_name: context.flock_name.as_deref(),
      observation: Some(observation),
      at,
      pigeon_url: &pigeon_url,
      manage_url: &manage_url,
    },
    clock_for(zone.as_ref()),
  );

  // One message per recipient rather than one with several addressees: a
  // bounce or a suppression on one address then costs only that delivery,
  // and nobody learns who else is on the alert. The transition itself is
  // still decided once, so the debounce fires for everyone or for no one.
  for recipient in &context.recipients {
    if let Err(e) = send_email_message(env, recipient, &message).await {
      console_error!("Alert email send failed for definition {}: {e}", def.id);
    }
  }
}

/// Whether this environment can send mail at all, by either transport --
/// lets callers with a graceful no-op path (e.g. org invites) log a link
/// instead of "sending" into the void, without duplicating the
/// binding/secret lookups.
pub(crate) fn email_configured(env: &Env) -> bool {
  env.send_email(EMAIL_BINDING).is_ok() || usesend_api_key(env).is_some()
}

/// `From:` address for platform mail. A sending domain is onboarded per
/// environment, so `MAIL_FROM_ADDRESS` ([env.*.vars], wrangler.toml)
/// overrides the useSend default where one differs.
fn mail_from_address(env: &Env) -> String {
  env
    .var("MAIL_FROM_ADDRESS")
    .map(|v| v.to_string())
    .ok()
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| DEFAULT_FROM_ADDRESS.to_string())
}

/// `RESEND_API_KEY` Worker secret, if configured -- mirrors
/// `helpers/greptime.rs::greptime_auth_token`'s secret-read shape. Never
/// set via `[vars]`, same rule this codebase enforces for every credential
/// (`wrangler secret put RESEND_API_KEY --env <env>`).
fn usesend_api_key(env: &Env) -> Option<String> {
  env
    .secret("RESEND_API_KEY")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty())
}

#[derive(serde::Serialize)]
struct UsesendEmailRequest<'a> {
  from: &'a str,
  to: [&'a str; 1],
  subject: &'a str,
  text: &'a str,
  #[serde(skip_serializing_if = "Option::is_none")]
  html: Option<&'a str>,
}

/// Domain-only form of an email address, for log lines that need to stay
/// diagnostic (spotting a bad domain or a bounce pattern) without retaining
/// a full recipient address now that `head_sampling_rate = 1`
/// (`wrangler.toml`) keeps every `console_error!`/`console_log!` line
/// instead of sampling almost all of them away. `send_via_usesend` is
/// shared by alert, invite, and feedback sends, so it has no per-call
/// context (alert definition id, org id, ...) to log instead of the
/// address -- redacting the address itself is the only option available at
/// this layer.
fn redact_email(email: &str) -> String {
  match email.rsplit_once('@') {
    Some((_, domain)) if !domain.is_empty() => format!("***@{domain}"),
    _ => "***@(unparseable)".to_string(),
  }
}

/// Plain-text only: what the ops-facing senders (feedback, contact, error
/// digests, allowance warnings) need.
pub(crate) async fn send_via_usesend(env: &Env, to: &str, subject: &str, text: &str) -> Result<()> {
  send_email(env, to, subject, text, None).await
}

/// Both parts of a formatted customer-facing message, so a client that
/// renders HTML gets the layout and one that does not gets the same words.
pub(crate) async fn send_email_message(env: &Env, to: &str, message: &EmailMessage) -> Result<()> {
  send_email(
    env,
    to,
    &message.subject,
    &message.text,
    Some(&message.html),
  )
  .await
}

/// One transactional email, Cloudflare Email Service first: the
/// `[[send_email]]` binding resolves only where it is declared, so its
/// absence is what routes an environment back to useSend's HTTP API.
async fn send_email(
  env: &Env,
  to: &str,
  subject: &str,
  text: &str,
  html: Option<&str>,
) -> Result<()> {
  let from = mail_from_address(env);
  match env.send_email(EMAIL_BINDING) {
    Ok(sender) => send_via_binding(&sender, &from, to, subject, text, html).await,
    Err(_) => post_via_usesend(env, &from, to, subject, text, html).await,
  }
}

/// Hands the message to Cloudflare Email Service, which signs it with the
/// sending domain's DKIM key and takes custody -- a resolved promise is
/// acceptance, not delivery.
async fn send_via_binding(
  sender: &SendEmail,
  from: &str,
  to: &str,
  subject: &str,
  text: &str,
  html: Option<&str>,
) -> Result<()> {
  let mut builder = SendEmailBuilder::builder(from, to, subject).text(text);
  if let Some(html) = html {
    builder = builder.html(html);
  }

  match sender.send_with_builder(&builder.build()).await {
    Ok(_) => {
      console_log!(
        "Email Service accepted mail to {} (subject: {subject})",
        redact_email(to)
      );
      Ok(())
    }
    Err(e) => {
      let reason = String::from(e.message());
      let mut message = String::with_capacity(29 + reason.len());
      message.push_str("Email Service send rejected: ");
      message.push_str(&reason);
      Err(Error::RustError(message))
    }
  }
}

/// POSTs one transactional email via useSend's Resend-compatible HTTP API
/// (`https://app.usesend.com/api/v1/emails`) -- mirrors
/// `helpers/greptime.rs::post_line_protocol`'s `Fetch`/`RequestInit`/header
/// shape. `RESEND_API_KEY` unset (expected until an operator runs
/// `wrangler secret put`) is treated the same way `greptime_auth_token`
/// being absent is treated elsewhere -- logged, never a hard failure,
/// since alert delivery is always best-effort.
async fn post_via_usesend(
  env: &Env,
  from: &str,
  to: &str,
  subject: &str,
  text: &str,
  html: Option<&str>,
) -> Result<()> {
  let Some(api_key) = usesend_api_key(env) else {
    console_error!(
      "RESEND_API_KEY not configured -- cannot send alert email to {} (subject: {subject})",
      redact_email(to)
    );
    return Ok(());
  };

  let body = UsesendEmailRequest {
    from,
    to: [to],
    subject,
    text,
    html,
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

  let req = Request::new_with_init("https://app.usesend.com/api/v1/emails", &init)?;
  let resp = Fetch::Request(req).send().await?;
  let status = resp.status_code();

  if status >= 400 {
    console_error!(
      "useSend send to {} returned HTTP {status} (subject: {subject})",
      redact_email(to)
    );
  } else {
    // The only positive signal on this path, and the reason it exists: every
    // other branch here logs exclusively on failure, so mail that went out
    // and mail that was never attempted are indistinguishable in a tail.
    // Confirming that alert delivery worked at all took a mailbox rather
    // than a log line. "Accepted" rather than "sent" on purpose -- a 2xx is
    // useSend taking custody, not the message reaching an inbox.
    console_log!(
      "useSend accepted mail to {} (subject: {subject})",
      redact_email(to)
    );
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::{ResolvedReading, evaluate_ingest_condition, redact_email, transitions_to_apply};
  use crate::objects::pigeons::PreviousTelemetryValue;
  use capsules::{AlertCondition, AlertObservation, Comparator};
  use std::collections::HashMap;

  fn decide(condition: &AlertCondition, reading: &ResolvedReading) -> Option<bool> {
    evaluate_ingest_condition(condition, reading).map(|(is_true, _)| is_true)
  }

  fn reading(at_secs: i64, key: &str, value: &str) -> ResolvedReading {
    ResolvedReading::new(
      at_secs,
      [(key.to_string(), value.to_string())].into_iter().collect(),
    )
  }

  fn with_previous(
    mut reading: ResolvedReading,
    key: &str,
    value: &str,
    at: i64,
  ) -> ResolvedReading {
    let mut previous = HashMap::new();
    previous.insert(
      key.to_string(),
      PreviousTelemetryValue {
        value: value.to_string(),
        reported_at: at,
      },
    );
    reading.previous = Some(previous);
    reading
  }

  fn threshold_over(key: &str, value: f64) -> AlertCondition {
    AlertCondition::Threshold {
      key: key.to_string(),
      comparator: Comparator::Gt,
      value,
    }
  }

  #[test]
  fn a_reading_without_the_key_decides_nothing() {
    let condition = threshold_over("temp", 30.0);
    assert_eq!(decide(&condition, &reading(100, "humidity", "40")), None);
  }

  #[test]
  fn a_non_numeric_value_decides_nothing_rather_than_recovering() {
    let condition = threshold_over("temp", 30.0);
    assert_eq!(decide(&condition, &reading(100, "temp", "warm")), None);
  }

  #[test]
  fn a_threshold_is_decided_per_reading() {
    let condition = threshold_over("temp", 30.0);
    assert_eq!(decide(&condition, &reading(100, "temp", "35")), Some(true));
    assert_eq!(decide(&condition, &reading(100, "temp", "25")), Some(false));
  }

  #[test]
  fn rate_of_change_measures_the_step_from_the_reading_before_it() {
    let condition = AlertCondition::RateOfChange {
      key: "temp".to_string(),
      max_delta: 10.0,
      window_secs: Some(60),
    };

    // Five degrees in ten seconds -- inside the batch this is one step of
    // a climb, not the whole climb.
    let gradual = with_previous(reading(110, "temp", "25"), "temp", "20", 100);
    assert_eq!(decide(&condition, &gradual), Some(false));

    // The same key jumping fifteen degrees in one step does fire.
    let jump = with_previous(reading(110, "temp", "35"), "temp", "20", 100);
    assert_eq!(decide(&condition, &jump), Some(true));
  }

  #[test]
  fn rate_of_change_says_nothing_on_a_first_sighting_or_across_a_long_gap() {
    let condition = AlertCondition::RateOfChange {
      key: "temp".to_string(),
      max_delta: 10.0,
      window_secs: Some(60),
    };

    assert_eq!(decide(&condition, &reading(110, "temp", "35")), None);

    let stale = with_previous(reading(1_000, "temp", "35"), "temp", "20", 100);
    assert_eq!(decide(&condition, &stale), None);
  }

  #[test]
  fn a_decided_reading_carries_what_it_saw() {
    let condition = threshold_over("temp", 30.0);
    assert_eq!(
      evaluate_ingest_condition(&condition, &reading(100, "temp", "35")),
      Some((true, AlertObservation::Value { observed: 35.0 }))
    );

    let condition = AlertCondition::RateOfChange {
      key: "temp".to_string(),
      max_delta: 10.0,
      window_secs: None,
    };
    let jump = with_previous(reading(110, "temp", "35"), "temp", "20", 100);
    assert_eq!(
      evaluate_ingest_condition(&condition, &jump),
      Some((
        true,
        AlertObservation::Change {
          previous: 20.0,
          observed: 35.0
        }
      ))
    );
  }

  #[test]
  fn a_steady_batch_costs_one_transition_at_its_end() {
    // Sixty-four true readings would be sixty-four round trips evaluated
    // naively; the run only needs its open and its close, and here those
    // are the same two calls that open the debounce window and fire it.
    let evaluated: Vec<(i64, bool, ())> = (0..64).map(|i| (1_000 + i * 10, true, ())).collect();
    assert_eq!(
      transitions_to_apply(&evaluated),
      vec![(1_000, true, ()), (1_630, true, ())]
    );
  }

  #[test]
  fn a_spike_that_rose_and_fell_inside_the_batch_is_still_applied() {
    let evaluated = vec![
      (100, false, "a"),
      (110, true, "b"),
      (120, true, "c"),
      (130, false, "d"),
      (140, false, "e"),
    ];
    assert_eq!(
      transitions_to_apply(&evaluated),
      vec![
        (100, false, "a"),
        (110, true, "b"),
        (130, false, "d"),
        (140, false, "e")
      ]
    );
  }

  #[test]
  fn a_single_reading_applies_exactly_once() {
    assert_eq!(
      transitions_to_apply(&[(100, true, ())]),
      vec![(100, true, ())]
    );
    assert!(transitions_to_apply::<()>(&[]).is_empty());
  }

  #[test]
  fn redact_email_keeps_domain_drops_local_part() {
    assert_eq!(redact_email("owner@example.com"), "***@example.com");
  }

  #[test]
  fn redact_email_handles_malformed_input_without_echoing_it() {
    assert_eq!(redact_email("not-an-email"), "***@(unparseable)");
    assert_eq!(redact_email("trailing@"), "***@(unparseable)");
  }
}
