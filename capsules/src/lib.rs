use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

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
