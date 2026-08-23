use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

// Shared by fancier's connection badge and dovecote's scheduled alert
// evaluator -- see that module's own doc comment for the rationale.
pub mod connection_state;

// Request/category types shared with fancier's feedback modal, plus the
// notification-email formatter dovecote's `POST /feedback` route uses.
// Re-exported at the crate root so consumers name them like every other
// capsules type.
pub mod feedback;
pub use feedback::{
  FeedbackCategory, FeedbackRequest, FeedbackSubmitter, MAX_FEEDBACK_BODY_BYTES,
  MAX_FEEDBACK_CONTACT_EMAIL_BYTES, MAX_FEEDBACK_DIAGNOSTICS_BYTES, MAX_FEEDBACK_MESSAGE_BYTES,
  MAX_FEEDBACK_PAGE_CONTEXT_BYTES, format_feedback_email,
};

// Client error-report envelope plus the normalizer/signature functions
// dovecote's `POST /errors` route and fancier's capture hooks share --
// see that module's own header for why the pure logic lives here.
pub mod errors;
pub use errors::{
  Breadcrumb, BreadcrumbKind, ErrorKind, ErrorNoteRequest, ErrorReport,
  MAX_ERROR_BREADCRUMB_DETAIL_BYTES, MAX_ERROR_BREADCRUMBS, MAX_ERROR_FIELD_BYTES,
  MAX_ERROR_MESSAGE_BYTES, MAX_ERROR_REPORT_BYTES, MAX_ERROR_REPORTS_PER_PAGE,
  MAX_ERROR_STACK_BYTES, SessionKind, error_signature, is_valid_build, normalize_message,
  normalize_route, truncate_bytes,
};

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
  /// Owning organization -- `Some` makes this an org-owned flock governed by
  /// `organization_members` roles; `None` keeps it a personal flock governed
  /// by `user_id` alone. Exactly one model applies at a time: once `org_id`
  /// is set, `user_id` is historical provenance (who created/transferred
  /// it), not an access grant -- see dovecote's
  /// `helpers/orgs.rs::authorize_flock`.
  #[serde(default)]
  pub org_id: Option<Uuid>,
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
      org_id: None,
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
  // This pigeon's own Zephyr `CONFIG_BOARD_TARGET` string, e.g.
  // "circuitdojo_feather/nrf9160/ns" -- operator-set at provisioning/update
  // time (device self-report may come later). `None` until an operator
  // tags it -- see `objects/pigeons.rs::check_firmware_board_compat` in
  // dovecote for where this is enforced against a firmware image's board.
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

/// User-definable forwarding target for a pigeon's telemetry: when set, the
/// queue consumer forwards each report to `url` as an InfluxDB line
/// protocol v2 HTTP write (GreptimeDB-compatible) instead of our own
/// `pigeon_telemetry_history` Postgres mirror — the DO's latest-value-per-key
/// `pigeon_telemetry` upsert always happens either way. Stored as JSON text
/// in the same column pattern as `connector` (no separate `*Row` variant
/// needed — no DB-native timestamp fields to convert). `auth_token` is
/// stripped on GET the same as `connector`'s `token`/`tls_psk_secret` — only
/// ever accepted on the dashboard PUT that sets it.
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
  // Operator-declared board at provisioning time -- optional, same
  // "unset until an operator tags it" story as `Pigeon::board`.
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
  // Same COALESCE/partial-update semantics as every other field here --
  // omitted keeps the current value, `Some` replaces it. No way to
  // explicitly clear an already-set board via this route today, same
  // limitation every other `Option<String>` field on this struct has.
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

// CoAP connector, terminated by `loft` on both transports: DTLS/UDP (coaps://, the primary --
// the scheme minted endpoints carry) and TLS/TCP (RFC 8323, coaps+tcp://); the sibling
// ~/pigeon Zephyr library speaks both, chosen at build time. tls_psk_secret
// is a 32-char hex PSK minted alongside `token` (one refresh rotates both together), NOT the
// token itself: RFC 4279 only requires stacks to support PSKs up to 64 bytes, mbedTLS's
// default MBEDTLS_PSK_MAX_LEN is 32, and libcoap's client caps at 64 -- the 92-char token
// would be unusable as a PSK on exactly the constrained stacks CoAP exists for. The PSK
// authenticates the DTLS/TLS handshake; `token` remains what authorizes every proxied device
// request upstream.
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

// --- Telemetry ---

/// Most distinct keys one pigeon's latest-value store will hold. The store
/// is a single JSON object in a single Durable Object SQLite row, so this
/// is what bounds that row against a device that renames its keys across
/// firmware versions: past the cap the least-recently-reported keys are
/// evicted, and a report carrying more than this many keys is refused
/// whole (`400`) rather than partly applied. Sized well above the device
/// library's own per-report ceiling (`~/pigeon`'s
/// `CONFIG_PIGEON_TELEMETRY_MAX_KEYS`, default 8, range max 64) so a
/// fleet's accumulated key set has room, while the worst-case row stays a
/// small fraction of the platform's 2 MB row limit.
pub const MAX_TELEMETRY_KEYS: usize = 128;

/// Longest single telemetry key accepted, in bytes. `~/pigeon` truncates
/// keys at 31 bytes (`PIGEON_TELEMETRY_KEY_MAX`); this leaves headroom for
/// other clients while keeping the worst-case stored row bounded.
pub const MAX_TELEMETRY_KEY_BYTES: usize = 128;

/// Longest single telemetry value accepted, in bytes. `~/pigeon` truncates
/// values at 127 bytes (`PIGEON_TELEMETRY_VAL_MAX`); the same headroom
/// rationale as `MAX_TELEMETRY_KEY_BYTES`. Values are opaque strings to
/// dovecote, so this is the only thing bounding one.
pub const MAX_TELEMETRY_VALUE_BYTES: usize = 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TelemetryLatest {
  pub key: String,
  pub value: String,
  #[serde(with = "time::serde::rfc3339")]
  pub reported_at: OffsetDateTime,
}

impl TelemetryLatest {
  /// The DO stores each key's timestamp as unix seconds (what `unixepoch()`
  /// wrote when the per-key table still existed, kept identical so the
  /// dashboard's deserialization never had to change), while the public
  /// shape is RFC 3339. This is the only conversion between the two.
  pub fn from_unix_seconds(key: String, value: String, reported_at: i64) -> Self {
    Self {
      key,
      value,
      reported_at: OffsetDateTime::from_unix_timestamp(reported_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
    }
  }
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

/// Most points either history read route will return for one request.
/// History stores one row per reported key, so a handful of keys at a
/// short interval reaches this within a day -- the cap is spent on the
/// newest points in range, and a response that hit it says so via
/// `TELEMETRY_HISTORY_TRUNCATED_HEADER`.
pub const TELEMETRY_HISTORY_MAX_POINTS: usize = 5000;

/// Set to `true`/`false` on every history response. The body stays a bare
/// `TelemetryHistoryPoint` array, so this is the only way a caller can
/// tell a complete range from the newest slice of a longer one -- an
/// absent header means a backend too old to report either way.
pub const TELEMETRY_HISTORY_TRUNCATED_HEADER: &str = "X-Telemetry-Truncated";

// Query params shared by both history read routes (GET
// /pigeons/:id/telemetry/history, GET /flocks/:id/telemetry/history).
// All optional: no key filter returns every key, no range returns
// everything within `TELEMETRY_HISTORY_MAX_POINTS`.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TelemetryHistoryQuery {
  pub key: Option<String>,
  /// Comma-separated keys, for a caller drawing several series at once (a
  /// GPS track needs `gps_lat,gps_lon` and nothing else). Separate from
  /// `key` rather than replacing it because query strings deserialize
  /// through `serde_urlencoded`, which has no repeated-parameter form --
  /// and because `key` is already a shipped parameter.
  pub keys: Option<String>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub since: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub until: Option<OffsetDateTime>,
  /// Opts into the pre-bucketing response shape: flat `TelemetryHistoryPoint`s
  /// (one per key per report), capped at `TELEMETRY_HISTORY_MAX_POINTS` and
  /// flagged via `TELEMETRY_HISTORY_TRUNCATED_HEADER` when a range holds
  /// more. The default (this absent/false) is bucketed instead --
  /// `TelemetryHistoryBucket`s, unbounded range, no truncation -- see that
  /// type's doc comment. This backstop exists for callers that need real
  /// per-report values rather than a bucket's aggregate: `fancier`'s GPS
  /// track widget needs `gps_lat`/`gps_lon` paired from the same report,
  /// which a bucket mean can't reconstruct, and the connection-badge
  /// "last seen" check needs the true latest timestamp, not a bucket's
  /// start. Both stay on `raw=true` deliberately -- see dovecote's
  /// `helpers/telemetry.rs`.
  #[serde(default)]
  pub raw: bool,
}

impl TelemetryHistoryQuery {
  /// The union of `key` and `keys` as one filter list, or `None` for "every
  /// key". Blank entries are dropped so a trailing comma or an empty
  /// `keys=` behaves like no filter at all rather than matching a key named
  /// "" and returning nothing.
  pub fn key_list(&self) -> Option<Vec<String>> {
    let mut keys: Vec<String> = self
      .key
      .iter()
      .map(String::as_str)
      .chain(self.keys.iter().flat_map(|k| k.split(',')))
      .map(str::trim)
      .filter(|k| !k.is_empty())
      .map(str::to_string)
      .collect();
    keys.sort();
    keys.dedup();
    (!keys.is_empty()).then_some(keys)
  }
}

#[cfg(test)]
mod telemetry_history_query_tests {
  use super::*;

  fn query(key: Option<&str>, keys: Option<&str>) -> TelemetryHistoryQuery {
    TelemetryHistoryQuery {
      key: key.map(str::to_string),
      keys: keys.map(str::to_string),
      ..Default::default()
    }
  }

  #[test]
  fn no_filter_means_every_key() {
    assert_eq!(query(None, None).key_list(), None);
  }

  #[test]
  fn blank_entries_do_not_become_a_filter() {
    assert_eq!(query(None, Some("")).key_list(), None);
    assert_eq!(query(None, Some(" , ,")).key_list(), None);
  }

  #[test]
  fn csv_is_split_and_trimmed() {
    assert_eq!(
      query(None, Some("gps_lat, gps_lon ,")).key_list(),
      Some(vec!["gps_lat".to_string(), "gps_lon".to_string()])
    );
  }

  #[test]
  fn single_key_and_csv_merge_without_duplicates() {
    assert_eq!(
      query(Some("gps_lat"), Some("gps_lon,gps_lat")).key_list(),
      Some(vec!["gps_lat".to_string(), "gps_lon".to_string()])
    );
  }
}

/// The default telemetry-history response bounds itself by BUCKETING
/// instead of truncating: a point is one key at one timestamp, so a
/// handful of keys reported every reporting cycle blows past
/// `TELEMETRY_HISTORY_MAX_POINTS` within a day regardless of how long a
/// range was asked for, and truncating always drops the same thing --
/// everything before the newest slice. Downsampling to a fixed target
/// bucket count instead makes every range drawable and the response size
/// roughly constant, at the cost of the bucket's own width standing in for
/// individual timestamps. `raw=true` (`TelemetryHistoryQuery::raw`) keeps
/// the old flat/truncating shape for callers that need real per-report
/// values.
pub const TELEMETRY_HISTORY_BUCKET_TARGET: usize = 360;

/// One time bucket's aggregate for one pigeon/key -- the bucketed history
/// routes' response element (`Vec<TelemetryHistoryBucket>`, ascending by
/// `bucket_start`, no cap and no truncation: see
/// `TELEMETRY_HISTORY_BUCKET_TARGET`'s doc comment for why bucketing makes
/// both unnecessary).
///
/// `min`/`max`/`mean` are `None` for a bucket whose values never parsed as
/// numeric (a non-numeric key, e.g. a firmware version string) -- `last`
/// still carries the raw string either way, since a bucket always has at
/// least one report backing it. `count` is how many reports landed in the
/// bucket, not how many of those were numeric.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TelemetryHistoryBucket {
  pub pigeon_id: String,
  pub key: String,
  #[serde(with = "time::serde::rfc3339")]
  pub bucket_start: OffsetDateTime,
  pub min: Option<f64>,
  pub max: Option<f64>,
  pub mean: Option<f64>,
  pub last: String,
  pub count: i64,
}

/// Bucket width (seconds) for a since/until range, aimed at
/// `TELEMETRY_HISTORY_BUCKET_TARGET` buckets across it. Floored at one
/// second -- a sub-second span (or a defensively-handled inverted range,
/// `until` before `since`) would otherwise divide down to zero or
/// negative, which `date_bin` can't use as a stride. Pure so the bucket
/// math is testable without a database; the SQL side (dovecote's
/// `helpers/telemetry.rs`) passes this straight into Postgres's
/// `make_interval(secs => ...)`.
pub fn telemetry_bucket_width_secs(since: OffsetDateTime, until: OffsetDateTime) -> f64 {
  let span = (until - since).as_seconds_f64();
  (span / TELEMETRY_HISTORY_BUCKET_TARGET as f64).max(1.0)
}

#[cfg(test)]
mod telemetry_bucket_width_tests {
  use super::*;
  use time::macros::datetime;

  #[test]
  fn one_day_divides_evenly() {
    let since = datetime!(2026-08-17 00:00:00 UTC);
    let until = datetime!(2026-08-18 00:00:00 UTC);
    assert_eq!(telemetry_bucket_width_secs(since, until), 240.0);
  }

  #[test]
  fn short_range_floors_at_one_second() {
    let since = datetime!(2026-08-17 00:00:00 UTC);
    let until = since + time::Duration::seconds(60);
    assert_eq!(telemetry_bucket_width_secs(since, until), 1.0);
  }

  #[test]
  fn zero_width_range_floors_at_one_second() {
    let t = datetime!(2026-08-17 00:00:00 UTC);
    assert_eq!(telemetry_bucket_width_secs(t, t), 1.0);
  }

  #[test]
  fn inverted_range_floors_at_one_second_instead_of_going_negative() {
    let since = datetime!(2026-08-17 00:00:00 UTC);
    let until = since - time::Duration::seconds(60);
    assert_eq!(telemetry_bucket_width_secs(since, until), 1.0);
  }

  #[test]
  fn thirteen_months_stays_a_sane_width() {
    let since = datetime!(2025-07-17 00:00:00 UTC);
    let until = datetime!(2026-08-17 00:00:00 UTC);
    let width = telemetry_bucket_width_secs(since, until);
    // ~13 months / 360 buckets is on the order of a day -- just a sanity
    // bound, not a pinned exact value that would break on leap-day drift.
    assert!(width > 60.0 * 60.0 * 12.0 && width < 60.0 * 60.0 * 48.0);
  }
}

// --- Device logs ---

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

// --- Device log dictionary ---

/// Size cap enforced by dovecote's `PUT /pigeons/:pigeon_id/log-dictionary`
/// route on an uploaded `log_dictionary.json` -- a real build's database is
/// tens-to-hundreds of KB (string mappings dominate; optional base64 ELF
/// string sections can add more), so 4MB is generous headroom, not a tuned
/// limit. Exported so the dashboard can pre-check a selected file without
/// duplicating the number, same convention as `MAX_FIRMWARE_BYTES` below.
pub const MAX_LOG_DICTIONARY_BYTES: usize = 4 * 1024 * 1024;

/// Response of `PUT /pigeons/:pigeon_id/log-dictionary` -- lightweight
/// metadata about the dictionary just stored, extracted server-side from the
/// uploaded JSON itself (never trusted separately from it). The dictionary
/// body is only ever read back via the `GET` route, which returns the raw
/// JSON document unwrapped -- Zephyr's own schema, not a capsules type, so
/// the dashboard's decoder and Zephyr's `log_parser.py` read the same bytes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LogDictionaryInfo {
  /// Stored size in bytes.
  pub size: i64,
  /// The database's own `build_id` field, if present -- ties the dictionary
  /// back to the firmware build that produced it (a dictionary only decodes
  /// the build it came from; a mismatched one yields garbage strings).
  pub build_id: Option<String>,
  /// The database's own `version` field (Zephyr dictionary DB format
  /// version, 3 as of Zephyr v4.x), if present.
  pub version: Option<i64>,
}

// --- Firmware / FOTA ---

/// Size cap enforced by dovecote's `POST /flocks/:flock_id/firmware` route
/// -- this fleet's signed MCUboot application images run ~300KB-1MB
/// (`~/pigeon-examples/build/dfu_application.zip`), so 2MB is generous
/// headroom, not a tuned limit. Exported so any future device-side or
/// dashboard-side caller can pre-check without duplicating the number.
pub const MAX_FIRMWARE_BYTES: usize = 2 * 1024 * 1024;

/// Shape embedded at `target_config.firmware` in a pigeon's shadow -- the
/// shadow-driven update signal. A nested object, not a flat key, since
/// Zephyr's `json_obj_parse` supports nested objects via
/// `JSON_OBJ_DESCR_OBJECT`, and old firmware ignores unknown top-level keys
/// either way, so this is backward-compatible with devices that predate
/// FOTA. `sha256` is lowercase hex (not base64) -- mbedTLS/PSA sha256 on the
/// device side naturally produces raw bytes to hex-compare, and hex is more
/// debuggable from the dashboard. This is also the exact response shape of
/// the DO-internal `/pigeon/device/firmware/target` route (see
/// `objects/pigeons.rs::get_firmware_target_device`), which the gateway's
/// `GET /device/pigeons/:id/firmware` route (`lib.rs`) uses to resolve
/// which R2 object to stream back -- the firmware bytes themselves never
/// pass through the pigeon's Durable Object (SQLite is not viable for
/// MB-sized blobs).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FirmwareTarget {
  pub version: String,
  pub size: i64,
  pub sha256: String,
}

/// One uploaded firmware image, catalogued per-flock in Postgres. Firmware
/// images are shared across every pigeon in a flock (same hardware fleet),
/// unlike per-pigeon state (connector, telemetry_endpoint, etc.) which lives
/// in that pigeon's own Durable Object -- flocks have no DO of their own
/// (see `Flock` above), so this catalog lives purely in Postgres, with no
/// `*Row` variant needed since Postgres hands back a native `OffsetDateTime`
/// directly (same as `Flock`). The actual binary lives in R2,
/// content-addressed by `sha256` (key `firmware/<sha256>.bin`) --
/// re-uploading identical bytes to the same flock (even under a new
/// `version` label) updates this row in place rather than duplicating the
/// R2 object. A pigeon's *assigned* firmware is a separate, per-pigeon
/// concern living in that pigeon's own shadow (see `FirmwareTarget` above),
/// not here.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FirmwareImage {
  pub id: Uuid,
  pub flock_id: Uuid,
  pub version: String,
  pub size: i64,
  pub sha256: String,
  // The Zephyr `CONFIG_BOARD_TARGET` string this image was built for (e.g.
  // "circuitdojo_feather/nrf9160/ns") -- required on every NEW upload via
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
/// `helpers/firmware.rs::sha256_hex`). `board` is required, unlike
/// `FirmwareImage::board` above being `Option` -- every NEW upload must
/// declare what it was built for; only pre-existing rows from before this
/// field existed are allowed to stay untagged.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FirmwareUploadQuery {
  pub version: String,
  pub board: String,
}

// --- Alerts ---
//
// Model follows docs/design/alerts-triggers.md §1, with one deliberate
// simplification (see `AlertCondition::MissingReport`'s own doc comment).
// `Threshold` and `RateOfChange` are both evaluated by dovecote's
// ingest-hook evaluator (`check_telemetry_alerts`,
// `dovecote/src/helpers/alerts.rs`); `DeviceState`/`MissingReport` are both
// evaluated by its Cron-Trigger-driven scheduled sweep instead
// (`evaluate_scheduled_alerts`, same file) -- see the design doc §2.2/§2.4
// for why absence-of-signal conditions can't be decided at ingest time.

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
/// exactly what `MissingReport` already models, and it needs different
/// semantics anyway (an `Unknown` pigeon has no `interval_secs` to compute
/// an age against). See design doc §1.1/§1.3.
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
/// (`helpers/alerts.rs::evaluate_scheduled_alerts`) -- see that function's
/// own doc comment for how it derives a pigeon's last-seen signal.
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
  /// No telemetry (any key) reported in at least `max_silence_secs` -- a
  /// simplification of the design doc's `MissingReport { key:
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
  /// key (design doc §1.1/§2.2). Edge-triggered, like `Threshold` -- a
  /// spike is only observable at the moment a new report lands next to the
  /// one before it, unlike `DeviceState`/`MissingReport`'s
  /// absence-of-signal checks. `window_secs`, if set, bounds how far apart
  /// the two samples may be: a gap larger than the window means the two
  /// reports aren't close enough in time to call the difference a "rate"
  /// of anything (e.g. a pigeon that was offline for a day and resumed at
  /// a very different reading is not a spike), so that comparison is
  /// skipped entirely rather than fired. `None` means no such bound --
  /// compare against the previous report regardless of how long ago it was.
  ///
  /// The "previous value" this needs doesn't live in any table --
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

/// Everything the public demo page is allowed to know about an alert, and
/// nothing else.
///
/// A separate struct rather than `AlertDefinition` with fields skipped:
/// that type carries `user_id` (a real account UUID) and `channel` (an
/// `AlertChannel::Email` holding a real address), and the demo route
/// answers anyone who asks it, with no session. A `#[serde(skip)]` leaves
/// those one careless edit away from being published; a struct that never
/// holds them cannot publish them at all. Same reasoning as the connector
/// token stripping the pigeon `GET` routes already do.
///
/// `key`/`comparator`/`value` are `Some` only for `AlertCondition::
/// Threshold`, the one condition carrying a number a chart can draw a line
/// at. Other conditions are still listed, with all three `None`, so the
/// page can say an alert exists rather than inventing a line for it or
/// pretending it isn't there.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DemoAlert {
  pub name: String,
  pub severity: AlertSeverity,
  pub status: AlertStatus,
  pub key: Option<String>,
  pub comparator: Option<Comparator>,
  pub value: Option<f64>,
}

impl DemoAlert {
  /// Takes the individual fields rather than an `AlertDefinition` so that
  /// this function has no access to the ones that must not be published —
  /// the restriction is in the signature, not in the author's memory.
  pub fn project(
    name: String,
    severity: AlertSeverity,
    status: AlertStatus,
    condition: &AlertCondition,
  ) -> Self {
    let (key, comparator, value) = match condition {
      AlertCondition::Threshold {
        key,
        comparator,
        value,
      } => (Some(key.clone()), Some(*comparator), Some(*value)),
      _ => (None, None, None),
    };

    Self {
      name,
      severity,
      status,
      key,
      comparator,
      value,
    }
  }
}

#[cfg(test)]
mod demo_alert_tests {
  use super::*;

  fn threshold() -> AlertCondition {
    AlertCondition::Threshold {
      key: "temp_c".into(),
      comparator: Comparator::Gt,
      value: 30.0,
    }
  }

  #[test]
  fn threshold_projects_the_numbers_a_chart_draws() {
    let alert = DemoAlert::project(
      "Too hot".into(),
      AlertSeverity::Warning,
      AlertStatus::Firing,
      &threshold(),
    );

    assert_eq!(alert.key.as_deref(), Some("temp_c"));
    assert_eq!(alert.comparator, Some(Comparator::Gt));
    assert_eq!(alert.value, Some(30.0));
    assert_eq!(alert.status, AlertStatus::Firing);
  }

  #[test]
  fn conditions_with_no_drawable_number_are_still_listed() {
    for condition in [
      AlertCondition::MissingReport {
        max_silence_secs: 600,
      },
      AlertCondition::DeviceState {
        state: ConnectionStateKind::Offline,
        min_duration_secs: None,
      },
      AlertCondition::RateOfChange {
        key: "temp_c".into(),
        max_delta: 5.0,
        window_secs: None,
      },
    ] {
      let alert = DemoAlert::project(
        "Went quiet".into(),
        AlertSeverity::Critical,
        AlertStatus::Ok,
        &condition,
      );

      assert_eq!(alert.name, "Went quiet");
      assert!(alert.key.is_none(), "{condition:?} leaked a key");
      assert!(alert.comparator.is_none(), "{condition:?} leaked an op");
      assert!(alert.value.is_none(), "{condition:?} leaked a value");
    }
  }

  /// A tripwire rather than a shape assertion. The route serving this type
  /// is unauthenticated, so any field added here is published to whoever
  /// asks; comparing the whole key set means a new field fails this test
  /// and has to be justified here before it can ship.
  #[test]
  fn serialized_form_carries_no_account_or_recipient_identifiers() {
    let json = serde_json::to_value(DemoAlert::project(
      "Too hot".into(),
      AlertSeverity::Warning,
      AlertStatus::Firing,
      &threshold(),
    ))
    .expect("DemoAlert serializes");

    let mut keys: Vec<&str> = json
      .as_object()
      .expect("DemoAlert serializes to a JSON object")
      .keys()
      .map(String::as_str)
      .collect();
    keys.sort_unstable();

    assert_eq!(
      keys,
      ["comparator", "key", "name", "severity", "status", "value"]
    );
  }
}

// --- Organizations & RBAC ---
//
// Shared-org access for teams (the PVTA departure-board case): individual
// Kratos accounts, one `organizations` row per team, membership rows in
// `organization_members` carrying a per-user role. A flock is EXACTLY one
// of user-owned (`Flock::org_id == None`) or org-owned (`Some`); an
// org-owned flock's pigeons additionally carry a `pigeon_acl` row whose
// `entity_id` IS the org id, so the existing per-pigeon ACL model extends
// to orgs without a new table -- see dovecote's `helpers/orgs.rs` (gateway
// side) and `objects/pigeons.rs::authorize_dashboard` (DO side) for the
// two centralized authorization helpers, and `docs/api.md`'s
// "Organizations" section for the full permission matrix.

/// A member's role within an organization. Stored as lowercase TEXT in
/// Postgres (`organization_members.role`, CHECK-constrained), serialized
/// lowercase on the wire.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrgRole {
  Owner,
  Admin,
  Member,
}

impl OrgRole {
  pub fn as_str(&self) -> &'static str {
    match self {
      OrgRole::Owner => "owner",
      OrgRole::Admin => "admin",
      OrgRole::Member => "member",
    }
  }

  /// Whether this role carries org-management rights (rename, invites,
  /// member removal, flock transfer target, owner-level pigeon rights on
  /// org-shared pigeons). `Member` is read/telemetry-level only -- see the
  /// permission matrix in `docs/api.md`.
  pub fn is_manager(&self) -> bool {
    matches!(self, OrgRole::Owner | OrgRole::Admin)
  }
}

impl std::str::FromStr for OrgRole {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "owner" => Ok(OrgRole::Owner),
      "admin" => Ok(OrgRole::Admin),
      "member" => Ok(OrgRole::Member),
      other => Err(format!("invalid org role '{other}'")),
    }
  }
}

impl std::fmt::Display for OrgRole {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// One org. Postgres hands back native `OffsetDateTime`s directly (same as
/// `Flock`/`FirmwareImage`), so no `*Row` variant is needed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Organization {
  pub id: Uuid,
  pub name: String,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
}

/// `GET /orgs` list item: an org plus the CALLER's own role in it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationMembership {
  pub organization: Organization,
  pub role: OrgRole,
}

/// One membership row (`organization_members`). `email` is denormalized at
/// join time (same convention as `flocks.owner_email`) so the dashboard can
/// show who a member is without a Kratos admin-API call from the edge;
/// `invited_by` is the inviting user's id (`None` for the founding owner),
/// giving the per-person audit trail the org model exists for.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationMember {
  pub org_id: Uuid,
  pub user_id: Uuid,
  pub role: OrgRole,
  pub email: Option<String>,
  pub invited_by: Option<Uuid>,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
}

/// One pending invite (`organization_invites`). The invite token itself is
/// NEVER stored or returned here -- only its sha256 hash is persisted, and
/// the cleartext token appears exactly once, in
/// `OrganizationInviteCreated::token` (write-once, same convention as
/// device connector tokens).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationInvite {
  pub id: Uuid,
  pub org_id: Uuid,
  pub email: String,
  pub role: OrgRole,
  #[serde(with = "time::serde::rfc3339")]
  pub expires_at: OffsetDateTime,
  pub created_by: Uuid,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
}

/// Response of `POST /orgs/:org_id/invites` -- the ONLY place the cleartext
/// invite token (and the ready-made accept URL built from it) is ever
/// returned; every later read (`GET /orgs/:org_id/invites`) carries only
/// the hash-backed `OrganizationInvite` metadata.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationInviteCreated {
  pub invite: OrganizationInvite,
  pub token: String,
  pub invite_url: String,
}

/// `GET /orgs/:org_id` -- members are visible to every member; `invites` is
/// populated only for owner/admin callers (empty vec otherwise).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationDetail {
  pub organization: Organization,
  pub caller_role: OrgRole,
  pub members: Vec<OrganizationMember>,
  pub invites: Vec<OrganizationInvite>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct OrganizationCreateRequest {
  pub name: String,
}

/// Body for `PUT /orgs/:org_id` (rename -- the only mutable org field).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct OrganizationRenameRequest {
  pub name: String,
}

/// Body for `PUT /orgs/:org_id/members/:user_id`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OrganizationMemberRoleUpdateRequest {
  pub role: OrgRole,
}

/// Body for `POST /orgs/:org_id/invites`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OrganizationInviteCreateRequest {
  pub email: String,
  pub role: OrgRole,
}

/// Body for `POST /orgs/invites/accept` -- token-alone (bearer) acceptance:
/// any authenticated session presenting a live, unconsumed token joins,
/// regardless of which email it registered under. Tradeoff vs. requiring
/// an email match is documented on the route in `docs/api.md`.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct OrganizationInviteAcceptRequest {
  pub token: String,
}

/// Body for `POST /flocks/:flock_id/transfer` -- moves a personal flock
/// into an org the caller manages (see `docs/api.md`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FlockTransferRequest {
  pub org_id: Uuid,
}

/// One entry of the internal `X-Org-Roles` header (gateway -> Durable
/// Object): the caller's org memberships as compact JSON
/// (`[{"id":"<uuid>","role":"owner"}]`), forwarded alongside `X-User-Id` so
/// the DO's ACL check can match org-granted `pigeon_acl` rows without its
/// own Postgres round trip. Internal wire shape, never exposed to clients.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct OrgRoleEntry {
  pub id: Uuid,
  pub role: OrgRole,
}

// --- Billing (Stripe) ---
//
// Billing attaches to `organizations`, not to `flocks`: an org is the only
// entity that already survives a change of individual owner and can hold a
// team's payment relationship. Usage aggregation stays in our own Postgres
// and Stripe's meter is a reporting sink, so nothing here treats Stripe as
// authoritative for anything we can compute ourselves -- these types carry
// only the state Stripe genuinely owns (who the customer is, whether the
// subscription is paid, when the period rolls).

/// The five subscription tiers. Stored lowercase in `organizations.plan`,
/// serialized lowercase on the wire. `Perch` is the free tier and the
/// resting state of an org that has never subscribed. Variants are declared
/// in ascending price order, which is what the derived `Ord` means -- it
/// lets the dashboard phrase a plan change as an upgrade or a downgrade
/// without carrying a separate rank table.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum BillingPlan {
  #[default]
  Perch,
  Builder,
  Growth,
  Scale,
  Fleet,
}

impl BillingPlan {
  pub fn as_str(&self) -> &'static str {
    match self {
      BillingPlan::Perch => "perch",
      BillingPlan::Builder => "builder",
      BillingPlan::Growth => "growth",
      BillingPlan::Scale => "scale",
      BillingPlan::Fleet => "fleet",
    }
  }

  /// Devices included before per-device overage applies. Perch is a hard
  /// cap rather than an overage floor -- the free tier never bills, it
  /// stops.
  pub fn included_devices(&self) -> i64 {
    match self {
      BillingPlan::Perch => 10,
      BillingPlan::Builder => 50,
      BillingPlan::Growth => 250,
      BillingPlan::Scale => 1_500,
      BillingPlan::Fleet => 10_000,
    }
  }

  /// Pooled device->platform messages included per month, account-wide.
  /// Counts telemetry reports, shadow report-backs and log uploads; shadow
  /// polls, firmware chunks, dashboard calls and WebSocket pings are not
  /// billable and never counted.
  pub fn included_messages(&self) -> i64 {
    match self {
      BillingPlan::Perch => 300_000,
      BillingPlan::Builder => 1_500_000,
      BillingPlan::Growth => 7_500_000,
      BillingPlan::Scale => 45_000_000,
      BillingPlan::Fleet => 300_000_000,
    }
  }

  /// Whether exceeding the message allowance bills overage or pauses
  /// ingestion. The free tier has no payment method to bill, so it is the
  /// one tier that must stop instead.
  pub fn bills_overage(&self) -> bool {
    !matches!(self, BillingPlan::Perch)
  }
}

impl std::str::FromStr for BillingPlan {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "perch" => Ok(BillingPlan::Perch),
      "builder" => Ok(BillingPlan::Builder),
      "growth" => Ok(BillingPlan::Growth),
      "scale" => Ok(BillingPlan::Scale),
      "fleet" => Ok(BillingPlan::Fleet),
      other => Err(format!("invalid billing plan '{other}'")),
    }
  }
}

impl std::fmt::Display for BillingPlan {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// Mirrors Stripe's own subscription status vocabulary verbatim, plus
/// `None` for an org that has never had a subscription. Kept verbatim
/// rather than collapsed into "paid/unpaid" because dunning distinguishes
/// them: `PastDue` is a customer Stripe is still retrying, `Unpaid` is one
/// it has given up on, and treating those the same would either cut off a
/// recoverable customer or keep serving an unrecoverable one.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
  #[default]
  None,
  Incomplete,
  IncompleteExpired,
  Trialing,
  Active,
  PastDue,
  Canceled,
  Unpaid,
  Paused,
}

impl SubscriptionStatus {
  pub fn as_str(&self) -> &'static str {
    match self {
      SubscriptionStatus::None => "none",
      SubscriptionStatus::Incomplete => "incomplete",
      SubscriptionStatus::IncompleteExpired => "incomplete_expired",
      SubscriptionStatus::Trialing => "trialing",
      SubscriptionStatus::Active => "active",
      SubscriptionStatus::PastDue => "past_due",
      SubscriptionStatus::Canceled => "canceled",
      SubscriptionStatus::Unpaid => "unpaid",
      SubscriptionStatus::Paused => "paused",
    }
  }

  /// Whether a paid tier's entitlements should currently be served.
  /// `PastDue` stays entitled deliberately: Stripe is still retrying the
  /// card, and cutting a fleet of devices off mid-dunning turns a recovered
  /// payment into a churned customer.
  pub fn is_entitled(&self) -> bool {
    matches!(
      self,
      SubscriptionStatus::Trialing | SubscriptionStatus::Active | SubscriptionStatus::PastDue
    )
  }
}

impl std::str::FromStr for SubscriptionStatus {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "none" => Ok(SubscriptionStatus::None),
      "incomplete" => Ok(SubscriptionStatus::Incomplete),
      "incomplete_expired" => Ok(SubscriptionStatus::IncompleteExpired),
      "trialing" => Ok(SubscriptionStatus::Trialing),
      "active" => Ok(SubscriptionStatus::Active),
      "past_due" => Ok(SubscriptionStatus::PastDue),
      "canceled" => Ok(SubscriptionStatus::Canceled),
      "unpaid" => Ok(SubscriptionStatus::Unpaid),
      "paused" => Ok(SubscriptionStatus::Paused),
      other => Err(format!("invalid subscription status '{other}'")),
    }
  }
}

impl std::fmt::Display for SubscriptionStatus {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// One price on a subscription item, as Stripe sends it. Only the two
/// fields that identify which tier was bought are modeled; everything else
/// on a Stripe price is Stripe's to know.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct StripePriceRow {
  #[serde(default)]
  pub id: String,
  /// The stable, human-chosen handle set when the price is created. This is
  /// what maps a Stripe price back to a `BillingPlan`, in preference to the
  /// generated `id`, so re-creating a price at a new amount doesn't orphan
  /// every subscription on it.
  #[serde(default)]
  pub lookup_key: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct StripeSubscriptionItemRow {
  /// The subscription item's own id (`si_…`) -- what the Subscriptions
  /// Update API addresses when swapping an item's price in place.
  #[serde(default)]
  pub id: String,
  #[serde(default)]
  pub price: StripePriceRow,
  /// Stripe API version 2026-07-29.dahlia moved the billing period bounds
  /// here from the subscription's own top level -- see
  /// `StripeSubscriptionRow::period_start`.
  #[serde(default)]
  pub current_period_start: Option<i64>,
  #[serde(default)]
  pub current_period_end: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct StripeSubscriptionItemsRow {
  #[serde(default)]
  pub data: Vec<StripeSubscriptionItemRow>,
}

/// A Stripe subscription object as it arrives on the wire -- the `*Row`
/// half of the pair, in the same sense as `PigeonRow`/`PigeonShadowRow`:
/// timestamps are unix-epoch integers and `status` is a bare string,
/// because that is what the source hands us. `OrganizationBilling` below is
/// the RFC 3339 / typed-enum public variant, produced by the `From` impl.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct StripeSubscriptionRow {
  pub id: String,
  pub customer: String,
  #[serde(default)]
  pub status: String,
  #[serde(default)]
  pub current_period_start: Option<i64>,
  #[serde(default)]
  pub current_period_end: Option<i64>,
  #[serde(default)]
  pub cancel_at_period_end: bool,
  #[serde(default)]
  pub items: StripeSubscriptionItemsRow,
  #[serde(default)]
  pub metadata: std::collections::HashMap<String, String>,
}

impl StripeSubscriptionRow {
  /// The item selling the licensed tier -- the one whose price
  /// `lookup_key` names a `BillingPlan`. A checkout-created subscription
  /// also carries metered overage items (message overage, device overage),
  /// and Stripe guarantees nothing about item order, so "the first item"
  /// can be a meter: tier resolution has to select by lookup_key, never by
  /// position.
  fn licensed_item(&self) -> Option<&StripeSubscriptionItemRow> {
    self.items.data.iter().find(|item| {
      item
        .price
        .lookup_key
        .as_deref()
        .is_some_and(|key| key.parse::<BillingPlan>().is_ok())
    })
  }

  /// Which tier this subscription sells, from `metadata.plan` first and the
  /// licensed item's price `lookup_key` second. `None` means the
  /// subscription named neither, in which case the caller must leave the
  /// org's stored plan alone -- guessing would silently downgrade a paying
  /// customer on a provisioning typo.
  pub fn plan(&self) -> Option<BillingPlan> {
    self
      .metadata
      .get("plan")
      .and_then(|raw| raw.parse().ok())
      .or_else(|| {
        self
          .licensed_item()
          .and_then(|item| item.price.lookup_key.as_deref())
          .and_then(|key| key.parse().ok())
      })
  }

  /// When the current billing period started. Stripe API version
  /// 2026-07-29.dahlia stopped sending this on the subscription itself and
  /// moved it onto the items, so this reads the licensed item first (the
  /// item this subscription's tier actually rides on), then any item that
  /// carries the field, then the subscription's own top level -- the only
  /// place an account still pinned to a pre-dahlia API version sends it.
  /// `items.data` being empty (or lacking the field) falls through the same
  /// way, rather than panicking.
  pub fn period_start(&self) -> Option<i64> {
    self
      .licensed_item()
      .and_then(|item| item.current_period_start)
      .or_else(|| {
        self
          .items
          .data
          .iter()
          .find_map(|item| item.current_period_start)
      })
      .or(self.current_period_start)
  }

  /// See `period_start`.
  pub fn period_end(&self) -> Option<i64> {
    self
      .licensed_item()
      .and_then(|item| item.current_period_end)
      .or_else(|| {
        self
          .items
          .data
          .iter()
          .find_map(|item| item.current_period_end)
      })
      .or(self.current_period_end)
  }

  /// The `si_…` id of the licensed tier item -- the item a plan change
  /// re-prices. Selection is by lookup_key, never by position, for the
  /// reason on `licensed_item`.
  pub fn licensed_item_id(&self) -> Option<&str> {
    self.licensed_item().map(|item| item.id.as_str())
  }

  /// The `si_…` id of the per-device overage item, whose rate is
  /// tier-specific (`device-overage-<tier>`) and so must be re-priced
  /// alongside the licensed item on a plan change. The pooled
  /// message-overage item shares one rate across tiers and is never
  /// touched. `None` on a subscription that predates the metered
  /// composition.
  pub fn device_overage_item_id(&self) -> Option<&str> {
    self
      .items
      .data
      .iter()
      .find(|item| {
        item
          .price
          .lookup_key
          .as_deref()
          .is_some_and(|key| key.starts_with("device-overage-"))
      })
      .map(|item| item.id.as_str())
  }
}

/// An organization's billing state: everything Stripe owns about the
/// subscription, in the shapes the rest of the codebase uses. `plan` is
/// optional for the reason given on `StripeSubscriptionRow::plan`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationBilling {
  pub plan: Option<BillingPlan>,
  pub status: SubscriptionStatus,
  pub stripe_customer_id: Option<String>,
  pub stripe_subscription_id: Option<String>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub current_period_start: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub current_period_end: Option<OffsetDateTime>,
  pub cancel_at_period_end: bool,
}

impl Default for OrganizationBilling {
  fn default() -> Self {
    OrganizationBilling {
      plan: Some(BillingPlan::Perch),
      status: SubscriptionStatus::None,
      stripe_customer_id: None,
      stripe_subscription_id: None,
      current_period_start: None,
      current_period_end: None,
      cancel_at_period_end: false,
    }
  }
}

impl From<StripeSubscriptionRow> for OrganizationBilling {
  fn from(row: StripeSubscriptionRow) -> Self {
    let plan = row.plan();
    // Resolved before `row.customer`/`row.id` move below -- both read all
    // of `row` by reference, so they have to run while it's still whole.
    let period_start = row.period_start();
    let period_end = row.period_end();
    OrganizationBilling {
      plan,
      // An unrecognized status string means Stripe added a state we don't
      // model yet. Falling back to `None` would read as "never subscribed"
      // and strip entitlements from a live customer, so hold `Incomplete`
      // instead: unentitled, but visibly a subscription in a state needing
      // a human.
      status: row.status.parse().unwrap_or(SubscriptionStatus::Incomplete),
      stripe_customer_id: Some(row.customer),
      stripe_subscription_id: Some(row.id),
      current_period_start: period_start.and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok()),
      current_period_end: period_end.and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok()),
      cancel_at_period_end: row.cancel_at_period_end,
    }
  }
}

/// Body of `POST /orgs/:org_id/billing/checkout` -- which paid tier the
/// caller wants a Stripe Checkout session for. The free tier is not
/// purchasable, so `plan: perch` is a 400 at the route.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BillingCheckoutRequest {
  pub plan: BillingPlan,
}

/// Body of `PUT /orgs/:org_id/billing/plan` -- which paid tier an already
/// subscribed org should move to. Separate from `BillingCheckoutRequest`
/// because the two requests' refusals differ: here `perch` means
/// cancellation (the portal's job, `400`) and the org's current tier is a
/// no-op (`409`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BillingPlanChangeRequest {
  pub plan: BillingPlan,
}

/// A hosted Stripe session URL (Checkout or the customer Billing Portal),
/// minted server-side for the dashboard to redirect to. Card data never
/// touches our own surfaces.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BillingSessionUrl {
  pub url: String,
}

/// `GET /orgs/:org_id/billing` -- an org's billing state plus its usage
/// against the allowance, in one read. `plan` is the stored tier;
/// `effective_plan` is the tier actually served (entitlement-gated, so a
/// cancelled org shows its old `plan` but an `effective_plan` of the free
/// tier). Usage-period bounds are the org's Stripe period while a live
/// subscription covers now, the calendar month otherwise -- matching how
/// usage itself is tallied.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OrganizationBillingOverview {
  pub plan: BillingPlan,
  pub status: SubscriptionStatus,
  pub entitled: bool,
  pub effective_plan: BillingPlan,
  pub cancel_at_period_end: bool,
  /// Whether a Stripe customer exists for this org -- the precondition for
  /// the Billing Portal button.
  pub has_billing_account: bool,
  #[serde(with = "time::serde::rfc3339")]
  pub usage_period_start: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub usage_period_end: OffsetDateTime,
  pub billable_messages: i64,
  pub included_messages: i64,
  pub device_count: i64,
  pub included_devices: i64,
}

/// Internal wire shape for the CoAP terminator's PSK resolution call --
/// dovecote's `GET /internal/coap-psk/:identity` (service-secret gated,
/// never CORS-exposed to browsers) returns this to `loft` (and only to
/// `loft`). `identity` is the pigeon's DO id
/// (`CoapConfig::tls_psk_identity`); `secret` is the short PSK its
/// DTLS/TLS handshake is keyed with; `token` is the pigeon's device bearer
/// token, which `loft` presents on every proxied `/device/pigeons/:id/*`
/// request -- the owning DO still verifies it cryptographically, so
/// possession grants exactly "act as this one device". Never returned by
/// any dashboard/device route; strip-on-read conventions for `Pigeon`
/// responses are unaffected.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CoapPskLookup {
  pub identity: String,
  pub secret: String,
  pub token: String,
}

#[cfg(test)]
mod billing_tests {
  use super::{BillingPlan, OrganizationBilling, StripeSubscriptionRow, SubscriptionStatus};

  // No top-level period fields: this is the shape Stripe actually sends
  // under API version 2026-07-29.dahlia and later, confirmed on a real
  // delivered `customer.subscription.updated`. Tests that care about the
  // period fields supply them explicitly, either on an item (current shape)
  // or at the top level (`legacy_subscription_json`, pre-dahlia shape).
  fn subscription_json(extra: &str) -> StripeSubscriptionRow {
    serde_json::from_str(&format!(
      r#"{{"id":"sub_123","customer":"cus_123","status":"active",
          "cancel_at_period_end":false{extra}}}"#
    ))
    .expect("subscription fixture should deserialize")
  }

  #[test]
  fn plan_comes_from_metadata_first() {
    let row = subscription_json(
      r#","metadata":{"plan":"growth"},
         "items":{"data":[{"price":{"id":"price_1","lookup_key":"scale"}}]}"#,
    );
    assert_eq!(row.plan(), Some(BillingPlan::Growth));
  }

  #[test]
  fn plan_falls_back_to_price_lookup_key() {
    let row =
      subscription_json(r#","items":{"data":[{"price":{"id":"price_1","lookup_key":"fleet"}}]}"#);
    assert_eq!(row.plan(), Some(BillingPlan::Fleet));
  }

  #[test]
  fn unnamed_or_unknown_plan_stays_none_rather_than_guessing() {
    assert_eq!(subscription_json("").plan(), None);
    assert_eq!(
      subscription_json(r#","metadata":{"plan":"enterprise"}"#).plan(),
      None
    );
    assert_eq!(
      subscription_json(r#","items":{"data":[{"price":{"id":"price_1"}}]}"#).plan(),
      None
    );
    // Only meters, no licensed tier item: still a refusal to guess.
    assert_eq!(
      subscription_json(
        r#","items":{"data":[
          {"price":{"id":"price_1","lookup_key":"message-overage"}},
          {"price":{"id":"price_2","lookup_key":"device-overage-growth"}}]}"#
      )
      .plan(),
      None
    );
  }

  #[test]
  fn plan_and_period_come_from_the_licensed_item_not_slot_zero() {
    // Checkout puts the metered overage prices on the same subscription,
    // and Stripe guarantees nothing about item order -- a meter in slot 0
    // must not decide the tier or the billing period.
    let row = subscription_json(
      r#","items":{"data":[
        {"price":{"id":"price_m","lookup_key":"message-overage"},
         "current_period_start":1,"current_period_end":2},
        {"price":{"id":"price_g","lookup_key":"growth"},
         "current_period_start":1754956800,"current_period_end":1757635200},
        {"price":{"id":"price_d","lookup_key":"device-overage-growth"}}]}"#,
    );
    assert_eq!(row.plan(), Some(BillingPlan::Growth));
    assert_eq!(row.period_start(), Some(1754956800));
    assert_eq!(row.period_end(), Some(1757635200));
  }

  #[test]
  fn period_without_a_licensed_item_falls_back_to_any_item_carrying_it() {
    // No tier item at all (metadata named the plan instead): the period is
    // still better read from an item than dropped, since only pre-dahlia
    // accounts send it at the top level.
    let row = subscription_json(
      r#","metadata":{"plan":"scale"},
         "items":{"data":[
        {"price":{"id":"price_m","lookup_key":"message-overage"},
         "current_period_start":1754956800,"current_period_end":1757635200}]}"#,
    );
    assert_eq!(row.plan(), Some(BillingPlan::Scale));
    assert_eq!(row.period_start(), Some(1754956800));
    assert_eq!(row.period_end(), Some(1757635200));
  }

  #[test]
  fn row_converts_to_typed_public_variant() {
    // Current wire shape: period bounds live on the item, not the
    // subscription. This is the exact shape a real staging webhook
    // delivered; a fixture with top-level period fields instead would pass
    // against code that can no longer read a real payload correctly.
    let billing: OrganizationBilling = subscription_json(
      r#","metadata":{"plan":"builder"},
         "items":{"data":[{"price":{"id":"price_1"},
           "current_period_start":1754956800,"current_period_end":1757635200}]}"#,
    )
    .into();
    assert_eq!(billing.plan, Some(BillingPlan::Builder));
    assert_eq!(billing.status, SubscriptionStatus::Active);
    assert_eq!(billing.stripe_customer_id.as_deref(), Some("cus_123"));
    assert_eq!(billing.stripe_subscription_id.as_deref(), Some("sub_123"));
    assert_eq!(
      billing.current_period_start.map(|t| t.unix_timestamp()),
      Some(1754956800)
    );
    assert_eq!(
      billing.current_period_end.map(|t| t.unix_timestamp()),
      Some(1757635200)
    );
    assert!(billing.status.is_entitled());
  }

  #[test]
  fn period_falls_back_to_subscription_level_for_older_api_versions() {
    // An account still pinned to a pre-dahlia Stripe API version sends
    // period bounds only at the top level -- no item carries them at all.
    let row = subscription_json(
      r#","current_period_start":1754956800,"current_period_end":1757635200,
         "items":{"data":[{"price":{"id":"price_1"}}]}"#,
    );
    assert_eq!(row.period_start(), Some(1754956800));
    assert_eq!(row.period_end(), Some(1757635200));
  }

  #[test]
  fn period_prefers_item_value_over_subscription_level() {
    // Both present and disagreeing: the item wins, since that's what the
    // current API version considers authoritative.
    let row = subscription_json(
      r#","current_period_start":1,"current_period_end":2,
         "items":{"data":[{"price":{"id":"price_1"},
           "current_period_start":1754956800,"current_period_end":1757635200}]}"#,
    );
    assert_eq!(row.period_start(), Some(1754956800));
    assert_eq!(row.period_end(), Some(1757635200));
  }

  #[test]
  fn empty_items_does_not_panic_and_falls_back() {
    let row = subscription_json(
      r#","current_period_start":1754956800,"current_period_end":1757635200,
         "items":{"data":[]}"#,
    );
    assert_eq!(row.period_start(), Some(1754956800));
    assert_eq!(row.period_end(), Some(1757635200));
    assert_eq!(subscription_json("").period_start(), None);
    assert_eq!(subscription_json("").period_end(), None);
  }

  #[test]
  fn unknown_status_holds_unentitled_instead_of_reading_as_never_subscribed() {
    let mut row = subscription_json("");
    row.status = "some_future_state".into();
    let billing: OrganizationBilling = row.into();
    assert_eq!(billing.status, SubscriptionStatus::Incomplete);
    assert!(!billing.status.is_entitled());
    assert_ne!(billing.status, SubscriptionStatus::None);
  }

  #[test]
  fn past_due_stays_entitled_while_stripe_retries() {
    assert!(SubscriptionStatus::PastDue.is_entitled());
    assert!(!SubscriptionStatus::Unpaid.is_entitled());
    assert!(!SubscriptionStatus::Canceled.is_entitled());
  }

  #[test]
  fn free_tier_pauses_rather_than_billing_overage() {
    assert!(!BillingPlan::Perch.bills_overage());
    assert!(BillingPlan::Builder.bills_overage());
  }

  #[test]
  fn plan_ordering_follows_the_tier_ladder() {
    // The derived Ord rides on variant declaration order; this pins that
    // order so a reordering refactor can't silently flip every
    // upgrade/downgrade phrasing in the dashboard.
    assert!(BillingPlan::Perch < BillingPlan::Builder);
    assert!(BillingPlan::Builder < BillingPlan::Growth);
    assert!(BillingPlan::Growth < BillingPlan::Scale);
    assert!(BillingPlan::Scale < BillingPlan::Fleet);
  }

  #[test]
  fn plan_change_item_ids_select_by_lookup_key_not_position() {
    // Meter in slot 0, tier in slot 1: the same ordering trap the plan
    // resolution guards against, now for the item ids a plan change
    // re-prices.
    let row = subscription_json(
      r#","items":{"data":[
        {"id":"si_msg","price":{"id":"price_m","lookup_key":"message-overage"}},
        {"id":"si_tier","price":{"id":"price_g","lookup_key":"growth"}},
        {"id":"si_dev","price":{"id":"price_d","lookup_key":"device-overage-growth"}}]}"#,
    );
    assert_eq!(row.licensed_item_id(), Some("si_tier"));
    assert_eq!(row.device_overage_item_id(), Some("si_dev"));
  }

  #[test]
  fn missing_items_yield_none_rather_than_grabbing_the_wrong_item() {
    // A licensed-only subscription (predates the metered composition) has
    // no device-overage item to re-price; the message-overage item must
    // never be mistaken for one.
    let row = subscription_json(
      r#","items":{"data":[
        {"id":"si_tier","price":{"id":"price_g","lookup_key":"growth"}},
        {"id":"si_msg","price":{"id":"price_m","lookup_key":"message-overage"}}]}"#,
    );
    assert_eq!(row.licensed_item_id(), Some("si_tier"));
    assert_eq!(row.device_overage_item_id(), None);
    assert_eq!(subscription_json("").licensed_item_id(), None);
    assert_eq!(subscription_json("").device_overage_item_id(), None);
  }
}
