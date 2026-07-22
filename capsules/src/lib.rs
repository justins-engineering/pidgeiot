use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

// Connection-state classification (task #31, moved here task #38) --
// shared by `fancier`'s connection badge and dovecote's scheduled alert
// evaluator. See that module's own doc comment for the full rationale.
pub mod connection_state;

#[macro_export]
macro_rules! unwrap_or_return_response {
  ($expr:expr) => {
    match $expr {
      Ok(val) => val,
      Err(err_resp) => return err_resp,
    }
  };
}

pub fn deserialize_unix_float_to_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let raw = f64::deserialize(deserializer)?;
  Ok(raw as i64)
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Flock {
  pub id: Uuid,
  pub user_id: Uuid,
  pub name: String,
  pub service_plan: String,
  pub pigeon_ids: Vec<String>,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
}

impl Default for Flock {
  fn default() -> Flock {
    Flock {
      id: Uuid::default(),
      user_id: Uuid::default(),
      name: String::with_capacity(64),
      service_plan: "free".to_string(),
      pigeon_ids: Vec::default(),
      updated_at: OffsetDateTime::UNIX_EPOCH,
      created_at: OffsetDateTime::UNIX_EPOCH,
    }
  }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct FlockCreateRequest {
  pub name: String,
}

impl Default for FlockCreateRequest {
  fn default() -> FlockCreateRequest {
    FlockCreateRequest {
      name: String::with_capacity(64),
    }
  }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct FlockUpdateRequest {
  pub name: String,
  pub service_plan: String,
}

impl Default for FlockUpdateRequest {
  fn default() -> FlockUpdateRequest {
    FlockUpdateRequest {
      name: String::with_capacity(64),
      service_plan: String::with_capacity(8),
    }
  }
}

// DB model — deserializes from SQLite's integer timestamps
#[derive(Deserialize, Debug)]
pub struct PigeonRow {
  pub id: String,
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: String,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub token_expires_at: i64,
  // JSON text like `connector`, NULL/absent when no user-defined endpoint is
  // configured — most pigeons never set this.
  pub telemetry_endpoint: Option<String>,
  // This pigeon's own Zephyr `CONFIG_BOARD_TARGET` string (task #20, phase
  // 1), e.g. "circuitdojo_feather/nrf9160/ns" -- operator-set at
  // provisioning/update time for now (device self-report is a later
  // hardening phase). `None` for every pigeon created before this field
  // existed, and for any pigeon an operator hasn't tagged yet -- see
  // `objects/pigeons.rs::check_firmware_board_compat` in `dovecote` for
  // where this is actually enforced against a firmware image's own board.
  pub board: Option<String>,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub updated_at: i64,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub created_at: i64,
}

impl From<PigeonRow> for Pigeon {
  fn from(row: PigeonRow) -> Self {
    Self {
      id: row.id,
      flock_id: row.flock_id,
      serial: row.serial,
      name: row.name,
      tags: row.tags,
      connector: serde_json::from_str(&row.connector).unwrap_or_default(),
      token_expires_at: OffsetDateTime::from_unix_timestamp(row.token_expires_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
      telemetry_endpoint: row
        .telemetry_endpoint
        .and_then(|s| serde_json::from_str(&s).ok()),
      board: row.board,
      updated_at: OffsetDateTime::from_unix_timestamp(row.updated_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
      created_at: OffsetDateTime::from_unix_timestamp(row.created_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
    }
  }
}

// API model — serializes/deserializes as RFC 3339
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Pigeon {
  pub id: String,
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: Connector,
  #[serde(with = "time::serde::rfc3339")]
  pub token_expires_at: OffsetDateTime,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub telemetry_endpoint: Option<TelemetryEndpoint>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub board: Option<String>,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
}

impl Default for Pigeon {
  fn default() -> Pigeon {
    Pigeon {
      id: String::with_capacity(64),
      flock_id: Uuid::default(),
      serial: None,
      name: None,
      tags: None,
      connector: Connector::default(),
      token_expires_at: OffsetDateTime::UNIX_EPOCH,
      telemetry_endpoint: None,
      board: None,
      updated_at: OffsetDateTime::UNIX_EPOCH,
      created_at: OffsetDateTime::UNIX_EPOCH,
    }
  }
}

/// User-definable forwarding target for a pigeon's telemetry (task #18):
/// when set, the queue consumer forwards each report to `url` as an
/// InfluxDB line protocol v2 HTTP write (GreptimeDB-compatible) instead of
/// our own `pigeon_telemetry_history` Postgres mirror — the DO's
/// latest-value-per-key `pigeon_telemetry` upsert always happens either
/// way. Stored as JSON text in the same column pattern as `connector`
/// (no separate `*Row` variant needed — it carries no DB-native timestamp
/// fields to convert). `auth_token` is stripped on GET the same as
/// `connector`'s `token`/`tls_psk_secret` — it's only ever accepted on the
/// dashboard PUT that sets it.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct TelemetryEndpoint {
  pub url: String,
  pub db: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub auth_token: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct PigeonTelemetryEndpointUpdateRequest {
  // `None` clears the endpoint (reverts to our own PG history); `Some`
  // sets/replaces it.
  pub telemetry_endpoint: Option<TelemetryEndpoint>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct PigeonCreateRequest {
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: Connector,
  // Operator-declared board at provisioning time (task #20, phase 1) --
  // optional, same "unset until an operator tags it" story as
  // `Pigeon::board`.
  #[serde(default)]
  pub board: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PigeonDetail {
  pub pigeon: Pigeon,
  pub acl: PigeonAcl,
  pub shadow: PigeonShadow,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct PigeonUpdateRequest {
  pub flock_id: Option<Uuid>,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: Option<Connector>,
  // Same `COALESCE`/partial-update semantics as every other field here --
  // omitted keeps the current value, `Some` replaces it. No way to
  // explicitly clear an already-set board via this route today, same
  // limitation every other `Option<String>` field on this struct already
  // has.
  #[serde(default)]
  pub board: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct PigeonAcl {
  pub entity_id: Uuid,
  pub role: String,
}

impl Default for PigeonAcl {
  fn default() -> PigeonAcl {
    PigeonAcl {
      entity_id: Uuid::default(),
      role: String::with_capacity(8),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PigeonAclUpdateRequest {
  pub entity_id: Uuid,
  pub role: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct JsonString(String);

impl JsonString {
  pub fn new(value: String) -> Result<Self, serde_json::Error> {
    serde_json::from_str::<serde_json::Value>(&value)?; // validate only
    Ok(Self(value))
  }

  pub fn into_inner(self) -> String {
    self.0
  }
}

impl std::fmt::Display for JsonString {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.0)
  }
}

impl JsonString {
  pub fn to_pretty(&self) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.0) {
      serde_json::to_string_pretty(&value).unwrap_or_else(|_| self.0.clone())
    } else {
      self.0.clone()
    }
  }
}

#[derive(Deserialize, Debug)]
pub struct PigeonShadowRow {
  pub target_version: i32,
  pub current_version: i32,
  pub target_config: JsonString,
  pub current_config: JsonString,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub updated_at: i64,
}

impl From<PigeonShadowRow> for PigeonShadow {
  fn from(row: PigeonShadowRow) -> Self {
    Self {
      target_version: row.target_version,
      current_version: row.current_version,
      target_config: row.target_config,
      current_config: row.current_config,
      updated_at: row.updated_at,
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonShadow {
  pub target_version: i32,
  pub current_version: i32,
  pub target_config: JsonString,
  pub current_config: JsonString,
  // Intentionally i64 unix seconds, not OffsetDateTime like other public API
  // variants in this crate: this field is parsed by device-side Zephyr firmware,
  // and a minimal wire size is a priority. Do not convert.
  pub updated_at: i64,
}

impl Default for PigeonShadow {
  fn default() -> PigeonShadow {
    PigeonShadow {
      target_version: i32::default(),
      current_version: i32::default(),
      target_config: JsonString("{}".to_string()),
      current_config: JsonString("{}".to_string()),
      updated_at: i64::default(),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PigeonShadowUpdateRequest {
  pub target_config: serde_json::Value,
}

// Device-facing report-back: the device echoes the `target_version` it just
// applied (read from an earlier shadow GET) alongside the resulting
// `current_config`, so the two stay associated even if `target_config`
// changes again before the device catches up.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PigeonShadowReportRequest {
  pub current_config: serde_json::Value,
  pub current_version: i32,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct HttpsConfig {
  pub endpoint: String,
  // Base64url-encoded Ed25519-signed binary bearer token (version | expires_at | signature),
  // not a JWT. Persisted as part of the DO's/Postgres's `connector` column, but stripped from
  // every API response except create/token-refresh (see dovecote's get/get_detail).
  pub token: String,
}

// CoAP-over-TLS/TCP (RFC 8323, coaps+tcp://), not CoAP-over-DTLS/UDP — matches the sibling
// ~/pigeon Zephyr device library, which has no on-device UDP support. tls_psk_secret currently
// mirrors `token` (both come from the same mint_device_credential() call), letting one refresh
// rotate both the bearer credential and the TLS-PSK secret together.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct CoapConfig {
  pub endpoint: String,
  pub token: String,
  pub tls_psk_identity: Option<String>,
  pub tls_psk_secret: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum Connector {
  Https(HttpsConfig),
  Coap(CoapConfig),
}

impl Default for Connector {
  fn default() -> Self {
    Connector::Https(HttpsConfig {
      endpoint: String::new(),
      token: String::new(),
    })
  }
}

// --- Telemetry (task #18) ---

// DB model for the DO's `pigeon_telemetry` latest-value-per-key table
// (SQLite integer timestamp, like PigeonRow/PigeonShadowRow above).
#[derive(Deserialize, Debug)]
pub struct TelemetryLatestRow {
  pub key: String,
  pub value: String,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub reported_at: i64,
}

impl From<TelemetryLatestRow> for TelemetryLatest {
  fn from(row: TelemetryLatestRow) -> Self {
    Self {
      key: row.key,
      value: row.value,
      reported_at: OffsetDateTime::from_unix_timestamp(row.reported_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TelemetryLatest {
  pub key: String,
  pub value: String,
  #[serde(with = "time::serde::rfc3339")]
  pub reported_at: OffsetDateTime,
}

// Postgres already hands back a native `OffsetDateTime` (unlike the DO's
// SQLite bindings), so `pigeon_telemetry_history` rows populate this
// directly — no `*Row` variant needed, same as `Flock`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TelemetryHistoryPoint {
  pub pigeon_id: String,
  pub key: String,
  pub value: String,
  pub value_num: Option<f64>,
  #[serde(with = "time::serde::rfc3339")]
  pub reported_at: OffsetDateTime,
}

// Query params shared by both history read routes (GET
// /pigeons/:id/telemetry/history, GET /flocks/:id/telemetry/history).
// All optional: no `key` returns every key, no range returns everything
// within the implicit LIMIT the route applies.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TelemetryHistoryQuery {
  pub key: Option<String>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub since: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub until: Option<OffsetDateTime>,
}

// --- Device logs (task #18) ---

/// Size cap enforced by dovecote's `POST /device/pigeons/:id/logs` route
/// (`objects/pigeons.rs::report_logs_device`) on a single log chunk body --
/// Zephyr `CONFIG_LOG_DICTIONARY_SUPPORT` records are compact by design, so
/// this is generous headroom, not a tuned limit. Exported so any future
/// device-side or dashboard-side caller can pre-check without duplicating
/// the number.
pub const MAX_LOG_CHUNK_BYTES: usize = 16 * 1024;

// DB model for the DO's `pigeon_log_chunks` bounded ring buffer (SQLite
// integer timestamp, like the other `*Row` types in this file). `data` is
// already base64 text in storage (see `objects/pigeons.rs`) -- same
// convention as `device_public_key`/device tokens elsewhere in this
// codebase -- so no bytes<->base64 conversion happens at this boundary.
#[derive(Deserialize, Debug)]
pub struct PigeonLogChunkRow {
  pub id: i64,
  pub data: String,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub received_at: i64,
}

impl From<PigeonLogChunkRow> for PigeonLogChunk {
  fn from(row: PigeonLogChunkRow) -> Self {
    Self {
      id: row.id,
      data: row.data,
      received_at: OffsetDateTime::from_unix_timestamp(row.received_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
    }
  }
}

/// One stored device dictionary-log chunk, returned base64-encoded for
/// host-side decode (`GET /pigeons/:id/logs`) -- the backend has no access
/// to the firmware's own dictionary/ELF needed to decode these itself; see
/// the sibling `~/pigeon` Zephyr library's `CLAUDE.md`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonLogChunk {
  pub id: i64,
  pub data: String,
  #[serde(with = "time::serde::rfc3339")]
  pub received_at: OffsetDateTime,
}

// --- Firmware / FOTA (task #23) ---

/// Size cap enforced by dovecote's `POST /flocks/:flock_id/firmware` route
/// -- this fleet's signed MCUboot application images run ~300KB-1MB
/// (`~/pigeon-examples/build/dfu_application.zip`), so 2MB is generous
/// headroom, not a tuned limit. Exported so any future device-side or
/// dashboard-side caller can pre-check without duplicating the number.
pub const MAX_FIRMWARE_BYTES: usize = 2 * 1024 * 1024;

/// Shape embedded at `target_config.firmware` in a pigeon's shadow (task
/// #23) -- the shadow-driven update signal. Coordinated with the device
/// client (`~/pigeon`/`~/pigeon-examples`) before being frozen: a nested
/// object, not a flat key, since Zephyr's `json_obj_parse` supports nested
/// objects via `JSON_OBJ_DESCR_OBJECT` and old firmware ignores unknown
/// top-level keys entirely either way (verified: `json_obj_parse` skips
/// them), so this is backward-compatible with devices that predate FOTA.
/// `sha256` is lowercase hex (not base64) -- mbedTLS/PSA sha256 on the
/// device side naturally produces raw bytes to hex-compare, and hex is more
/// debuggable from the dashboard. This is also the exact response shape of
/// the DO-internal `/pigeon/device/firmware/target` route (see
/// `objects/pigeons.rs::get_firmware_target_device`), which the gateway's
/// `GET /device/pigeons/:id/firmware` route (`lib.rs`) uses to resolve
/// which R2 object to stream back -- the firmware bytes themselves never
/// pass through the pigeon's Durable Object (SQLite is not acceptable for
/// MB-sized blobs; see this workspace's root `CLAUDE.md`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FirmwareTarget {
  pub version: String,
  pub size: i64,
  pub sha256: String,
}

/// One uploaded firmware image, catalogued per-flock in Postgres (task
/// #23). Firmware images are shared across every pigeon in a flock (same
/// hardware fleet), unlike per-pigeon state (connector, telemetry_endpoint,
/// etc.) which lives in that pigeon's own Durable Object -- flocks already
/// have no DO of their own (see `Flock` above), so this catalog lives
/// purely in Postgres, with no `*Row` variant needed since Postgres hands
/// back a native `OffsetDateTime` directly (same as `Flock`). The actual
/// binary lives in R2, content-addressed by `sha256` (key
/// `firmware/<sha256>.bin`) -- re-uploading identical bytes to the same
/// flock (even under a new `version` label) updates this row in place
/// rather than duplicating the R2 object. A pigeon's *assigned* firmware is
/// a separate, per-pigeon concern living in that pigeon's own shadow (see
/// `FirmwareTarget` above), not here.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FirmwareImage {
  pub id: Uuid,
  pub flock_id: Uuid,
  pub version: String,
  pub size: i64,
  pub sha256: String,
  // The Zephyr `CONFIG_BOARD_TARGET` string this image was built for (e.g.
  // "circuitdojo_feather/nrf9160/ns") -- required on every NEW upload
  // (task #20, phase 1: firmware/board geometry compatibility) via
  // `FirmwareUploadQuery::board` below, but `Option` here since
  // pre-existing catalog rows predate the column and stay untagged
  // (`NULL`) until an operator retags them. Enforced against
  // `Pigeon::board` before a shadow assignment is accepted -- see
  // `objects/pigeons.rs::check_firmware_board_compat` in `dovecote`.
  pub board: Option<String>,
  #[serde(with = "time::serde::rfc3339")]
  pub uploaded_at: OffsetDateTime,
}

/// Query params for `POST /flocks/:flock_id/firmware` -- `size`/`sha256`
/// are deliberately absent: both are computed server-side from the
/// uploaded bytes, never trusted from the client (see
/// `helpers/firmware.rs::sha256_hex`). `board` (task #20, phase 1) is
/// required, unlike `FirmwareImage::board` above being `Option` -- every
/// NEW upload must declare what it was built for; only pre-existing rows
/// from before this field existed are allowed to stay untagged.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FirmwareUploadQuery {
  pub version: String,
  pub board: String,
}

// --- Alerts (task #32, extended task #38, #39) ---
//
// Model follows docs/design/alerts-triggers.md §1, with one deliberate
// simplification (see `AlertCondition::MissingReport`'s own doc comment).
// `Threshold` and `RateOfChange` (task #39) are both evaluated by
// dovecote's ingest-hook evaluator (`check_telemetry_alerts`,
// `dovecote/src/helpers/alerts.rs`); `DeviceState`/`MissingReport` are both
// evaluated by its Cron-Trigger-driven scheduled sweep instead
// (`evaluate_scheduled_alerts`, same file, task #38) -- see the design doc
// §2.2/§2.4 for why absence-of-signal conditions can't be decided at
// ingest time. `RateOfChange` needed a "previous value" lookup this
// codebase hadn't built yet as of task #38 (design doc §2.2) -- see that
// variant's own doc comment below for how `check_telemetry_alerts` now
// sources it (read-before-overwrite in the DO, not a second history
// round-trip).

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Comparator {
  Gt,
  Gte,
  Lt,
  Lte,
  Eq,
}

impl Comparator {
  pub fn evaluate(&self, observed: f64, threshold: f64) -> bool {
    match self {
      Comparator::Gt => observed > threshold,
      Comparator::Gte => observed >= threshold,
      Comparator::Lt => observed < threshold,
      Comparator::Lte => observed <= threshold,
      Comparator::Eq => observed == threshold,
    }
  }
}

impl Default for Comparator {
  fn default() -> Self {
    Comparator::Eq
  }
}

/// Mirrors `fancier::helpers::connection_state::ConnectionState` today,
/// minus `Unknown` -- an alert on "we've never heard from this pigeon" is
/// exactly what `MissingReport` already models (once it exists), and it
/// needs different semantics anyway (an `Unknown` pigeon has no
/// `interval_secs` to compute an age against). See design doc §1.1/§1.3.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum ConnectionStateKind {
  Offline,
  Stale,
}

/// A boolean predicate over one pigeon's (or one flock's) observable state
/// (design doc §1.1). `Threshold` and `RateOfChange` are both fully
/// evaluated by `check_telemetry_alerts` at every telemetry ingest;
/// `DeviceState` and `MissingReport` are both absence-of-signal conditions
/// by definition (design doc §2.4) -- "went offline/stale" or "nothing
/// arrived in N seconds" can't be usefully decided at the moment a report
/// just arrived (that arrival itself proves the pigeon is online), so
/// neither is evaluated by the ingest-triggered hook. Both are instead
/// evaluated by dovecote's Cron-Trigger-driven scheduled sweep
/// (`helpers/alerts.rs::evaluate_scheduled_alerts`, task #38) -- see that
/// function's own doc comment for how it derives a pigeon's last-seen
/// signal.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AlertCondition {
  Threshold {
    key: String,
    comparator: Comparator,
    value: f64,
  },
  DeviceState {
    state: ConnectionStateKind,
    min_duration_secs: Option<i64>,
  },
  /// No telemetry (any key) reported in at least `max_silence_secs` --
  /// task #38's simplification of the design doc's `MissingReport { key:
  /// Option<String>, window_secs: i64 }` sketch (§1.1): dropping the
  /// optional per-key scoping keeps this a straightforward "heartbeat"
  /// check (has this pigeon reported *anything* recently) rather than a
  /// per-metric absence check, which `Threshold` combined with a
  /// dashboard-side "hasn't crossed in a while" isn't really a fit for
  /// anyway. Evaluated the same way as `DeviceState` -- see
  /// `evaluate_scheduled_alerts`'s doc comment (`dovecote/src/helpers/alerts.rs`).
  MissingReport { max_silence_secs: i64 },
  /// Fires when `key`'s numeric value has moved by more than `max_delta`
  /// (`|new - old| > max_delta`) since the previous report of that same
  /// key (task #39, design doc §1.1/§2.2). Edge-triggered, like
  /// `Threshold` -- a spike is only observable at the moment a new report
  /// lands next to the one before it, unlike `DeviceState`/`MissingReport`'s
  /// absence-of-signal checks. `window_secs`, if set, bounds how far apart
  /// the two samples may be: a gap larger than the window means the two
  /// reports aren't close enough in time to call the difference a "rate"
  /// of anything (e.g. a pigeon that was offline for a day and resumed at
  /// a very different reading is not a spike), so that comparison is
  /// skipped entirely rather than fired. `None` means no such bound --
  /// compare against the previous report regardless of how long ago it was.
  ///
  /// The "previous value" this needs doesn't live in any table today --
  /// `pigeon_telemetry` (the DO's own store) is latest-value-per-key, and
  /// the incoming report's own UPSERT overwrites the only copy before an
  /// evaluator could otherwise read it. `dovecote::objects::pigeons` solves
  /// this by reading each key's current row immediately before its UPSERT
  /// runs (`read_previous_telemetry`), carrying the result alongside the
  /// new values (`TelemetryWriteResult::previous_values`) to wherever
  /// `check_telemetry_alerts` ends up running -- no second table, no extra
  /// history-store round trip. A key with no previous row (this pigeon's
  /// first-ever report of it) simply has no entry to compare against, so
  /// this condition can never fire on a first reading.
  RateOfChange {
    key: String,
    max_delta: f64,
    window_secs: Option<i64>,
  },
}

impl Default for AlertCondition {
  fn default() -> Self {
    AlertCondition::Threshold {
      key: String::new(),
      comparator: Comparator::default(),
      value: 0.0,
    }
  }
}

/// Delivery channel for a fired/cleared alert (design doc §3). `Email` is
/// the only variant today -- kept as an enum (rather than a bare struct) so
/// adding `Webhook`/`Sms`/`Push` later is additive, matching how
/// `Connector` already lets `Pigeon` support more than one protocol without
/// a rewrite. `to: None` means "use the owning flock's stored
/// `owner_email`" (design doc §3.4); `Some` is an explicit per-alert
/// override.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AlertChannel {
  Email { to: Option<String> },
}

impl Default for AlertChannel {
  fn default() -> Self {
    AlertChannel::Email { to: None }
  }
}

/// Mirrors ThingsBoard's alarm severity framing (design doc §2.3) -- carried
/// through to the notification email's subject/badge color. Stored as plain
/// `TEXT` in Postgres (not JSONB like `condition`/`channel`), so this has
/// its own `FromStr`/`as_str` rather than going through `serde_json`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum AlertSeverity {
  Warning,
  Critical,
}

impl AlertSeverity {
  pub fn as_str(&self) -> &'static str {
    match self {
      AlertSeverity::Warning => "warning",
      AlertSeverity::Critical => "critical",
    }
  }
}

impl std::str::FromStr for AlertSeverity {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "warning" => Ok(AlertSeverity::Warning),
      "critical" => Ok(AlertSeverity::Critical),
      other => Err(format!("invalid alert severity '{other}'")),
    }
  }
}

impl Default for AlertSeverity {
  fn default() -> Self {
    AlertSeverity::Warning
  }
}

/// Which pigeon(s) an `AlertDefinition` applies to (design doc §1.2) --
/// mutually exclusive, mirrors how `Connector`/`TelemetryEndpoint` are
/// already per-pigeon while `FirmwareImage` is already per-flock in this
/// same codebase. A flock-scoped alert evaluates independently per pigeon
/// currently in that flock (one `AlertState` row per `(definition_id,
/// pigeon_id)` -- see `AlertState` below), not one combined state for the
/// whole flock.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AlertScope {
  Pigeon(String),
  Flock(Uuid),
}

/// DB model for one row of Postgres's `alert_definitions` table (design doc
/// §1.4) -- `condition`/`channel` arrive as `::text`-cast JSONB (see
/// `dovecote/src/helpers/alerts.rs`, which SELECTs them cast to text since
/// this workspace's `tokio-postgres` isn't built with the
/// `with-serde_json-1` feature), `severity` as its own plain-text column.
/// Postgres already hands back a native `OffsetDateTime` for
/// `TIMESTAMPTZ` columns (unlike the DO's SQLite bindings elsewhere in this
/// crate), so no epoch-float `deserialize_with` is needed here, same as
/// `Flock`/`FirmwareImage`.
#[derive(Deserialize, Debug)]
pub struct AlertDefinitionRow {
  pub id: Uuid,
  pub user_id: Uuid,
  pub flock_id: Option<Uuid>,
  pub pigeon_id: Option<String>,
  pub name: String,
  pub condition: String,
  pub severity: String,
  pub channel: String,
  pub enabled: bool,
  pub created_at: OffsetDateTime,
  pub updated_at: OffsetDateTime,
}

impl From<AlertDefinitionRow> for AlertDefinition {
  fn from(row: AlertDefinitionRow) -> Self {
    // Postgres's CHECK constraint (see init-db.sql) guarantees exactly one
    // of pigeon_id/flock_id is set for any real row -- the (None, None) arm
    // below should be unreachable, but falls back to an empty pigeon scope
    // rather than panicking, matching this crate's existing
    // permissive-on-malformed-stored-data convention (e.g. PigeonRow's
    // `connector` parse).
    let scope = match (row.pigeon_id, row.flock_id) {
      (Some(id), _) => AlertScope::Pigeon(id),
      (None, Some(id)) => AlertScope::Flock(id),
      (None, None) => AlertScope::Pigeon(String::new()),
    };

    Self {
      id: row.id,
      user_id: row.user_id,
      scope,
      name: row.name,
      condition: serde_json::from_str(&row.condition).unwrap_or_default(),
      severity: row.severity.parse().unwrap_or_default(),
      channel: serde_json::from_str(&row.channel).unwrap_or_default(),
      enabled: row.enabled,
      created_at: row.created_at,
      updated_at: row.updated_at,
    }
  }
}

/// Public API model for one user-defined alert (design doc §1.4) --
/// Postgres-only, not DO-mirrored (same reasoning already applied to
/// `FirmwareImage`: this is dashboard-authored config with no device-facing
/// counterpart, and a flock-scoped alert has no DO to live in at all).
/// Debounce/fired-state deliberately does NOT live on this struct -- see
/// `AlertState` below for why a flock-scoped alert needs one state row per
/// pigeon it applies to, not one shared state on the definition itself.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AlertDefinition {
  pub id: Uuid,
  pub user_id: Uuid,
  pub scope: AlertScope,
  pub name: String,
  pub condition: AlertCondition,
  pub severity: AlertSeverity,
  pub channel: AlertChannel,
  pub enabled: bool,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
}

/// Body for `POST /pigeons/:pigeon_id/alerts` and `POST
/// /flocks/:flock_id/alerts` -- scope is deliberately NOT part of this
/// request body; it's implied by which route was hit (and which
/// owner-gate, `PigeonAccess`/`FlockAccess`, already passed), not trusted
/// from the client.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AlertDefinitionCreateRequest {
  pub name: String,
  pub condition: AlertCondition,
  #[serde(default)]
  pub severity: AlertSeverity,
  pub channel: AlertChannel,
}

/// Body for `PUT /alerts/:alert_id` -- `None` keeps the current value for
/// that field, same partial-update convention as `PigeonUpdateRequest`.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AlertDefinitionUpdateRequest {
  pub name: Option<String>,
  pub condition: Option<AlertCondition>,
  pub severity: Option<AlertSeverity>,
  pub channel: Option<AlertChannel>,
  pub enabled: Option<bool>,
}

/// Debounce/hysteresis + fired-state tracking (design doc §2.3) -- one row
/// per `(alert_definition_id, pigeon_id)` pair, NOT per definition, because
/// a flock-scoped alert fires/clears independently per pigeon it applies to
/// (five pigeons going offline is five clear notifications, not one
/// ambiguous one). `status` mirrors ThingsBoard's raise/clear alarm
/// lifecycle: `Ok -> Firing` only once the condition has been continuously
/// true for the definition's own debounce window, sending exactly one
/// "fired" email on that transition; `Firing -> Ok` sends exactly one
/// "cleared" email on the reverse transition. No `*Row` variant needed --
/// same as `Flock`/`FirmwareImage`, Postgres hands back native
/// `OffsetDateTime`s directly.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum AlertStatus {
  Ok,
  Firing,
}

impl AlertStatus {
  pub fn as_str(&self) -> &'static str {
    match self {
      AlertStatus::Ok => "ok",
      AlertStatus::Firing => "firing",
    }
  }
}

impl std::str::FromStr for AlertStatus {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "ok" => Ok(AlertStatus::Ok),
      "firing" => Ok(AlertStatus::Firing),
      other => Err(format!("invalid alert status '{other}'")),
    }
  }
}

impl Default for AlertStatus {
  fn default() -> Self {
    AlertStatus::Ok
  }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AlertState {
  pub alert_definition_id: Uuid,
  pub pigeon_id: String,
  pub status: AlertStatus,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub first_true_at: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub last_notified_at: Option<OffsetDateTime>,
}
