use crate::helpers::{FlockAccess, PigeonAccess, get_db_client};
use crate::objects::pigeons::PreviousTelemetryValue;
use capsules::connection_state::{self, ConnectionState};
use capsules::{
  AlertChannel, AlertCondition, AlertDefinition, AlertDefinitionRow, AlertDefinitionUpdateRequest,
  AlertScope, AlertState, AlertStatus, ConnectionStateKind, JsonString,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use tokio_postgres::{Client, Row, types::Type};
use uuid::Uuid;
use worker::{Env, Error, Fetch, Method, Request, RequestInit, Result, console_error};

/// Column list shared by every `alert_definitions` read/RETURNING statement
/// -- `condition`/`channel` are cast to `::text` rather than read as native
/// JSONB because this workspace's `tokio-postgres` isn't built with the
/// `with-serde_json-1` feature (see `Cargo.toml`). Every other JSONB
/// column in this codebase is only ever written, never read back through
/// `tokio-postgres` directly, so this cast is the read-side mirror of the
/// `$N::jsonb` write pattern those columns already use.
const ALERT_DEFINITION_COLUMNS: &str = "id, user_id, flock_id, pigeon_id, name, \
  condition::text AS condition, severity, channel::text AS channel, enabled, \
  created_at, updated_at";

/// Fixed debounce window before a continuously-true condition transitions
/// `Ok -> Firing`. Scaling this per-pigeon off `telemetry_interval` the
/// way `connection_state::classify` (`capsules::connection_state`) already
/// does is a reasonable follow-up, not yet done -- a single fixed window
/// is a deliberate simplification, not an oversight.
const ALERT_DEBOUNCE_SECS: i64 = 60;

/// `From:` address for alert emails sent via useSend -- shares the
/// platform's one verified sending domain with Kratos's courier setup, but
/// never the credential. The Worker secret is still NAMED
/// `RESEND_API_KEY` for historical reasons but holds a useSend API key --
/// useSend speaks the Resend-shaped payload, so sends 401 against
/// api.resend.com if this ever gets swapped for a real Resend key (see
/// `send_via_usesend` below).
const USESEND_FROM_ADDRESS: &str = "alerts@noreply.pidgeiot.com";

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

/// Evaluation hook -- called alongside `write_telemetry_default` at each
/// of its three call sites (`queue.rs::dispatch_to_do`,
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
pub async fn check_telemetry_alerts(
  env: &Env,
  pigeon_id: &str,
  metrics: &HashMap<String, String>,
  previous: &HashMap<String, PreviousTelemetryValue>,
  reported_at_ms: u64,
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

    let is_true = match &def.condition {
      AlertCondition::Threshold {
        key,
        comparator,
        value,
      } => {
        let Some(raw) = metrics.get(key) else {
          continue;
        };
        let Ok(observed) = raw.parse::<f64>() else {
          continue;
        };
        comparator.evaluate(observed, *value)
      }
      AlertCondition::RateOfChange {
        key,
        max_delta,
        window_secs,
      } => {
        let Some(raw) = metrics.get(key) else {
          continue;
        };
        let Ok(observed) = raw.parse::<f64>() else {
          continue;
        };
        // No previous row for this key -- either this pigeon's first-ever
        // report of it, or the previous value wasn't numeric. Either way,
        // nothing to diff against yet, so this can never fire on a first
        // reading (capsules::AlertCondition::RateOfChange's own doc
        // comment).
        let Some(prev) = previous.get(key) else {
          continue;
        };
        let Ok(prev_value) = prev.value.parse::<f64>() else {
          continue;
        };
        if let Some(window) = window_secs {
          let gap_secs = (reported_at_ms / 1000) as i64 - prev.reported_at;
          if gap_secs > *window {
            // The two samples are too far apart in time to call their
            // difference a "rate" of anything (e.g. resuming after a long
            // silence at a very different reading is not a spike) -- skip
            // rather than fire.
            continue;
          }
        }
        (observed - prev_value).abs() > *max_delta
      }
      // DeviceState/MissingReport (and any future absence-of-signal
      // variant) aren't ingest-evaluable here -- see AlertCondition's doc
      // comment in capsules and evaluate_scheduled_alerts below.
      AlertCondition::DeviceState { .. } | AlertCondition::MissingReport { .. } => continue,
    };

    if let Err(e) = apply_alert_transition(&client, env, &def, pigeon_id, is_true).await {
      console_error!(
        "Alert transition failed for definition {} / pigeon {pigeon_id}: {e}",
        def.id
      );
    }
  }

  Ok(())
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

      if let Err(e) = apply_alert_transition(&client, env, &def, &pigeon_id, is_true).await {
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
      // Already firing -- no periodic re-notify implemented (would be an
      // optional cooldown-gated re-send, off by default).
    }
  }

  Ok(())
}

/// Resolves who an alert's notification email should go to: the channel's
/// own explicit override if set, otherwise the owning flock's stored
/// `owner_email` -- resolved via this definition's own `flock_id` if
/// flock-scoped, or via its pigeon's `flock_id` if pigeon-scoped.
/// `owner_email` is populated by `lib.rs`'s
/// `require_auth_session`/`helpers/flocks.rs` (`create_user_flock` on
/// create, `backfill_owner_email` opportunistically on `GET /flocks`) from
/// the session's own `identity.traits.email` -- a flock whose owner has
/// never authenticated since can still resolve to `None` here, and
/// `send_alert_email` logs that clearly rather than silently dropping the
/// notification.
async fn resolve_alert_recipient(client: &Client, def: &AlertDefinition) -> Option<String> {
  let AlertChannel::Email { to } = &def.channel;

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

  let owner_email: Option<String> = result.ok().and_then(|row| row.get("owner_email"));

  // Defense-in-depth: the create/update routes already reject an override
  // that isn't the caller's own verified address, but definitions that
  // predate that validation (or rows written outside the API) could still
  // carry an arbitrary `to`. At send time an override is honored only if
  // it matches the flock's owner_email -- anything else falls back to the
  // owner with a log line, so alert delivery can never be aimed at an
  // address the platform hasn't tied to this account.
  if let Some(explicit) = to {
    match &owner_email {
      Some(owner) if owner.eq_ignore_ascii_case(explicit.trim()) => {
        return Some(explicit.clone());
      }
      _ => {
        console_error!(
          "Alert '{}' ({}): ignoring email override that doesn't match the flock owner_email; delivering to owner instead",
          def.name,
          def.id
        );
      }
    }
  }

  owner_email
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

  if let Err(e) = send_via_usesend(env, &recipient, &subject, &text).await {
    console_error!("Alert email send failed for definition {}: {e}", def.id);
  }
}

/// Whether the useSend transport is configured for this environment --
/// lets callers with a graceful no-op path (e.g. org invites) log a link
/// instead of "sending" into the void, without duplicating the
/// secret-read logic.
pub(crate) fn usesend_configured(env: &Env) -> bool {
  usesend_api_key(env).is_some()
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
}

/// POSTs one transactional email via useSend's Resend-compatible HTTP API
/// (`https://app.usesend.com/api/v1/emails`) -- mirrors
/// `helpers/greptime.rs::post_line_protocol`'s `Fetch`/`RequestInit`/header
/// shape. `RESEND_API_KEY` unset (expected until an operator runs
/// `wrangler secret put`) is treated the same way `greptime_auth_token`
/// being absent is treated elsewhere -- logged, never a hard failure,
/// since alert delivery is always best-effort.
pub(crate) async fn send_via_usesend(env: &Env, to: &str, subject: &str, text: &str) -> Result<()> {
  let Some(api_key) = usesend_api_key(env) else {
    console_error!(
      "RESEND_API_KEY not configured -- cannot send alert email to {to} (subject: {subject})"
    );
    return Ok(());
  };

  let body = UsesendEmailRequest {
    from: USESEND_FROM_ADDRESS,
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

  let req = Request::new_with_init("https://app.usesend.com/api/v1/emails", &init)?;
  let resp = Fetch::Request(req).send().await?;

  if resp.status_code() >= 400 {
    console_error!(
      "Resend send to {to} returned HTTP {} (subject: {subject})",
      resp.status_code()
    );
  }

  Ok(())
}
