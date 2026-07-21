use capsules::TelemetryHistoryPoint;
use std::collections::HashMap;
use time::OffsetDateTime;
use worker::{Env, Fetch, Method, Request, RequestInit, Result, console_error};

/// Bare origin (scheme + host + port, e.g. `http://127.0.0.1:4000` in dev)
/// of the platform's own default GreptimeDB instance (task #26) — `None`
/// means "not configured for this environment," which is the expected
/// state for staging/prod until the user stands up the self-hosted
/// instance behind a Cloudflare Tunnel and sets this var (see
/// `wrangler.toml`'s comments). Every caller in this module treats an
/// unset origin as "fall back to Postgres," never as an error — this is
/// the grace-period design the task's design doc settled on.
pub fn greptime_origin(env: &Env) -> Option<String> {
  env
    .var("GREPTIMEDB_ENDPOINT")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty())
}

/// Optional GreptimeDB **database name** (`GREPTIMEDB_DB` var, per-env) the
/// platform-default read/write paths target. `None` → GreptimeDB's built-in
/// `public` database (dev and prod both use it as-is). Staging sets this to
/// its own name (e.g. `"staging"`) so it can share prod's single self-hosted
/// instance without its telemetry landing in prod's `public` db — the
/// isolation the task #26 design doc (§1.4) and `wrangler.toml` call for.
/// Only ever applied to our own `GREPTIMEDB_ENDPOINT` origin, never a
/// per-pigeon `telemetry_endpoint`. NOTE: GreptimeDB's InfluxDB write does
/// NOT auto-create a missing database (it 400s "Failed to find schema"), so
/// a non-`public` name here must be `CREATE DATABASE`'d once at setup — see
/// `wrangler.toml`. Until it is, `write_telemetry_default` just falls back to
/// Postgres history, so telemetry is never lost, only un-isolated.
fn greptime_db(env: &Env) -> Option<String> {
  env
    .var("GREPTIMEDB_DB")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty())
}

/// Optional bearer token for GreptimeDB's own HTTP auth, if the deployment
/// has one configured — a Worker secret (`wrangler secret put
/// GREPTIMEDB_AUTH_TOKEN`), never a plaintext var. Local dev's docker
/// GreptimeDB runs with no auth at all (`standalone start`, no
/// `--user-provider`), so this is simply absent there and every caller
/// already treats `None` as "send no Authorization header."
fn greptime_auth_token(env: &Env) -> Option<String> {
  env
    .secret("GREPTIMEDB_AUTH_TOKEN")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty())
}

/// Cloudflare Access service-token headers for reaching the tunneled
/// staging/prod GreptimeDB host — both Worker secrets, both required
/// together or not sent at all. **Only ever attached to requests aimed at
/// our own `GREPTIMEDB_ENDPOINT` origin** (`write_greptime_default`,
/// `query_greptime_sql` below) — never to a per-pigeon user-configured
/// `telemetry_endpoint` (`queue.rs::forward_line_protocol`), which can be
/// any URL a dashboard user sets. Attaching our own tunnel credentials to
/// an arbitrary user-supplied URL would leak them; the two code paths are
/// kept structurally separate (different functions, `extra_headers`
/// explicitly `&[]` at the per-pigeon call site) specifically so this
/// can't happen by accident.
fn greptime_access_headers(env: &Env) -> Vec<(String, String)> {
  let id = env
    .secret("GREPTIMEDB_ACCESS_CLIENT_ID")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty());
  let secret = env
    .secret("GREPTIMEDB_ACCESS_CLIENT_SECRET")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty());

  match (id, secret) {
    (Some(id), Some(secret)) => vec![
      ("CF-Access-Client-Id".to_string(), id),
      ("CF-Access-Client-Secret".to_string(), secret),
    ],
    _ => Vec::new(),
  }
}

/// Line protocol escaping for measurement/tag/field keys and tag values:
/// commas, spaces, and equals signs must be backslash-escaped outside of
/// quoted string field values (order matters -- backslash itself first, so
/// the later replacements' own backslashes aren't re-escaped). Shared by
/// both line-protocol write paths (`queue.rs`'s per-pigeon forward and
/// `write_greptime_default` below) -- moved here from `queue.rs` (task
/// #26) so the two don't duplicate this escaping logic.
pub fn escape_key_or_tag(value: &str) -> String {
  value
    .replace('\\', "\\\\")
    .replace(',', "\\,")
    .replace(' ', "\\ ")
    .replace('=', "\\=")
}

/// Line protocol escaping for a quoted string field value: backslashes and
/// double quotes only (commas/spaces/equals need no escaping inside
/// quotes).
pub fn escape_field_string(value: &str) -> String {
  value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Minimal percent-encoding for a URL query value / form-urlencoded body
/// value -- avoids pulling in the `url` crate's query-encoding just for
/// these few call sites (a user-supplied `db` name, and this module's own
/// `sql=` query-string bodies).
pub fn url_encode_component(value: &str) -> String {
  let mut out = String::with_capacity(value.len());
  for b in value.bytes() {
    match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
      _ => out.push_str(&format!("%{b:02X}")),
    }
  }
  out
}

/// Builds one InfluxDB line protocol v2 line for a single device telemetry
/// report -- one line, one point in time: every key in `metrics` becomes a
/// field on a single `pigeon_telemetry` measurement, tagged by `pigeon_id`.
/// Shared by `queue.rs`'s per-pigeon forward and `write_greptime_default`
/// below (task #26) -- previously duplicated inline in `queue.rs` before
/// this platform-default write path existed.
pub fn build_line_protocol(
  pigeon_id: &str,
  metrics: &HashMap<String, String>,
  reported_at_ms: u64,
) -> String {
  let mut line = String::with_capacity(48 + metrics.len() * 24);
  line.push_str("pigeon_telemetry,pigeon_id=");
  line.push_str(&escape_key_or_tag(pigeon_id));
  line.push(' ');

  for (i, (key, value)) in metrics.iter().enumerate() {
    if i > 0 {
      line.push(',');
    }
    line.push_str(&escape_key_or_tag(key));
    line.push('=');
    match value.parse::<f64>() {
      Ok(n) => line.push_str(&n.to_string()),
      Err(_) => {
        line.push('"');
        line.push_str(&escape_field_string(value));
        line.push('"');
      }
    }
  }

  line.push(' ');
  line.push_str(&reported_at_ms.to_string());
  line
}

/// POSTs a pre-built line-protocol line to `url`. `extra_headers` exists
/// specifically for `write_greptime_default`'s CF-Access headers -- see
/// that function's doc comment and `greptime_access_headers`'s above for
/// why those must never reach `queue.rs::forward_line_protocol`'s
/// per-pigeon call site (which always passes `&[]`).
pub async fn post_line_protocol(
  url: &str,
  line: &str,
  auth_token: Option<&str>,
  extra_headers: &[(String, String)],
) -> Result<()> {
  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  init.body = Some(line.to_string().into());
  init.headers.set("Content-Type", "text/plain; charset=utf-8")?;
  if let Some(token) = auth_token {
    init.headers.set("Authorization", &format!("Token {token}"))?;
  }
  for (name, value) in extra_headers {
    init.headers.set(name, value)?;
  }

  let req = Request::new_with_init(url, &init)?;
  let resp = Fetch::Request(req).send().await?;

  if resp.status_code() >= 400 {
    return Err(worker::Error::RustError(format!(
      "line-protocol write to '{url}' returned {}",
      resp.status_code()
    )));
  }

  Ok(())
}

/// Forwards one device telemetry report to the platform's own default
/// GreptimeDB instance (task #26) -- the new default write path, used
/// whenever `GREPTIMEDB_ENDPOINT` is configured for this environment.
/// Errors (including "not configured") are returned to the caller rather
/// than swallowed here, since `write_telemetry_default` below is what
/// decides whether to fall back to Postgres.
async fn write_greptime_default(
  env: &Env,
  pigeon_id: &str,
  metrics: &HashMap<String, String>,
  reported_at_ms: u64,
) -> Result<()> {
  let Some(origin) = greptime_origin(env) else {
    return Err(worker::Error::RustError(
      "GREPTIMEDB_ENDPOINT not configured".into(),
    ));
  };

  let line = build_line_protocol(pigeon_id, metrics, reported_at_ms);
  let url = match greptime_db(env) {
    Some(db) => format!(
      "{origin}/v1/influxdb/write?db={}&precision=ms",
      url_encode_component(&db)
    ),
    None => format!("{origin}/v1/influxdb/write?precision=ms"),
  };
  let token = greptime_auth_token(env);
  let extra_headers = greptime_access_headers(env);

  post_line_protocol(&url, &line, token.as_deref(), &extra_headers).await
}

/// Grace-period default telemetry write (task #26): tries the platform's
/// own GreptimeDB instance first (if `GREPTIMEDB_ENDPOINT` is configured
/// for this environment), falling back to the Postgres
/// `pigeon_telemetry_history` table on either an unset endpoint OR a
/// forward error -- see the task's design doc (`## 3`) for why this
/// fallback exists (a brand-new self-hosted single instance has no uptime
/// track record yet) and why it's a temporary grace-period measure, not a
/// permanent dual-write architecture: once Greptime forwarding succeeds,
/// this returns without touching Postgres at all.
///
/// Shared by all three "no per-pigeon `telemetry_endpoint` override" write
/// sites -- the queue consumer (`queue.rs`), and the two no-queue-bound
/// fallbacks task #17 closed the gap on (`objects/pigeons.rs`'s
/// `handle_ws_telemetry`/`report_telemetry_device`) -- so all three keep
/// behaving identically, which was the whole point of task #17.
pub async fn write_telemetry_default(
  env: &Env,
  pigeon_id: &str,
  metrics: &HashMap<String, String>,
  reported_at_ms: u64,
) -> Result<()> {
  if greptime_origin(env).is_some() {
    match write_greptime_default(env, pigeon_id, metrics, reported_at_ms).await {
      Ok(()) => return Ok(()),
      Err(e) => {
        console_error!(
          "Greptime forward failed for pigeon {pigeon_id}, falling back to PG history: {e}"
        );
      }
    }
  }

  crate::helpers::write_telemetry_history(env, pigeon_id, metrics).await
}

#[derive(serde::Deserialize)]
struct GreptimeSqlResponse {
  #[serde(default)]
  output: Vec<GreptimeOutput>,
}

#[derive(serde::Deserialize)]
struct GreptimeOutput {
  records: Option<GreptimeRecords>,
}

#[derive(serde::Deserialize)]
struct GreptimeRecords {
  schema: GreptimeSchema,
  rows: Vec<Vec<serde_json::Value>>,
}

#[derive(serde::Deserialize)]
struct GreptimeSchema {
  column_schemas: Vec<GreptimeColumnSchema>,
}

#[derive(serde::Deserialize)]
struct GreptimeColumnSchema {
  name: String,
}

/// Only hex characters, matching the fixed shape of a pigeon's own DO-ID
/// string everywhere else in this codebase -- validated before ever being
/// interpolated into a raw SQL string below, since GreptimeDB's HTTP SQL
/// endpoint has no bind-parameter mechanism the way `tokio_postgres` does.
fn is_valid_pigeon_id(id: &str) -> bool {
  !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// `pigeon_telemetry` is a GreptimeDB **auto-schema wide table**, not a
/// row-per-key log like Postgres's `pigeon_telemetry_history` -- confirmed
/// empirically against a real local instance while building this (not
/// assumed from docs): every distinct metric key a device has ever
/// reported becomes its own `FIELD` column (`pigeon_id` is the tag/`PRI`
/// key, `greptime_timestamp` the `TIMESTAMP`/`PRI` key), auto-added on
/// first appearance via `write_greptime_default`'s line-protocol writes,
/// `NULL` for any row/timestamp that didn't report that particular key.
/// `SELECT *` fetches every column; `key`/`since`/`until` filtering and
/// the 5000-point cap are applied Rust-side after pivoting each wide row
/// back into one `TelemetryHistoryPoint` per non-null field -- there is no
/// SQL-level way to filter by "key" here since a key is a column, not a
/// value.
fn build_history_sql(
  pigeon_ids: &[String],
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> String {
  let ids_list = pigeon_ids
    .iter()
    .map(|id| format!("'{id}'"))
    .collect::<Vec<_>>()
    .join(",");

  let mut sql = format!("SELECT * FROM pigeon_telemetry WHERE pigeon_id IN ({ids_list})");
  if let Some(dt) = since {
    sql.push_str(&format!(
      " AND greptime_timestamp >= {}",
      dt.unix_timestamp_nanos()
    ));
  }
  if let Some(dt) = until {
    sql.push_str(&format!(
      " AND greptime_timestamp <= {}",
      dt.unix_timestamp_nanos()
    ));
  }
  // Row-level cap, not point-level: each row can pivot into multiple
  // points (one per reported key), so the final Vec is truncated to 5000
  // again after pivoting to preserve the same "at most 5000 points"
  // contract the Postgres-backed read path has always had.
  sql.push_str(" ORDER BY greptime_timestamp ASC LIMIT 5000;");
  sql
}

/// Runs one SQL-over-HTTP query (`POST {origin}/v1/sql`) against the
/// platform's default GreptimeDB and pivots the wide-table response into
/// `TelemetryHistoryPoint`s. "Table not found" (confirmed empirically: a
/// `400` with `{"code":4001,"error":"...Table not found..."}`) is treated
/// as an empty result, not an error -- it's the expected shape for any
/// environment/pigeon before its first-ever telemetry write has landed,
/// and treating it as a hard error would spuriously trigger the
/// fallback-to-PG path (in `query_greptime_history_for_pigeon(s)` below)
/// for what is actually the common, harmless "no data yet" case.
async fn query_greptime_sql(env: &Env, sql: &str) -> Result<Vec<TelemetryHistoryPoint>> {
  let Some(origin) = greptime_origin(env) else {
    return Err(worker::Error::RustError(
      "GREPTIMEDB_ENDPOINT not configured".into(),
    ));
  };

  let url = format!("{origin}/v1/sql");
  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  // `db` (if set) rides alongside `sql` as a second form field — the same
  // isolation the write path applies via `?db=` (see `greptime_db`). Unset
  // → GreptimeDB queries its default `public` database.
  let mut body = format!("sql={}", url_encode_component(sql));
  if let Some(db) = greptime_db(env) {
    body.push_str("&db=");
    body.push_str(&url_encode_component(&db));
  }
  init.body = Some(body.into());
  init
    .headers
    .set("Content-Type", "application/x-www-form-urlencoded")?;
  if let Some(token) = greptime_auth_token(env) {
    init.headers.set("Authorization", &format!("Token {token}"))?;
  }
  for (name, value) in greptime_access_headers(env) {
    init.headers.set(&name, &value)?;
  }

  let req = Request::new_with_init(&url, &init)?;
  let mut resp = Fetch::Request(req).send().await?;
  let status = resp.status_code();
  let text = resp.text().await?;

  if status >= 400 {
    if text.contains("Table not found") {
      return Ok(Vec::new());
    }
    return Err(worker::Error::RustError(format!(
      "Greptime SQL query failed ({status}): {text}"
    )));
  }

  let parsed: GreptimeSqlResponse = serde_json::from_str(&text).map_err(|e| {
    console_error!("Greptime SQL response parse error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  let Some(records) = parsed.output.into_iter().next().and_then(|o| o.records) else {
    return Ok(Vec::new());
  };

  let columns: Vec<String> = records
    .schema
    .column_schemas
    .into_iter()
    .map(|c| c.name)
    .collect();

  let mut points = Vec::new();
  for row in records.rows {
    let mut row_pigeon_id: Option<String> = None;
    let mut reported_at: Option<OffsetDateTime> = None;

    for (col, val) in columns.iter().zip(row.iter()) {
      match col.as_str() {
        "pigeon_id" => row_pigeon_id = val.as_str().map(|s| s.to_string()),
        "greptime_timestamp" => {
          reported_at = val
            .as_i64()
            .and_then(|ns| OffsetDateTime::from_unix_timestamp_nanos(ns as i128).ok());
        }
        _ => {}
      }
    }

    let (Some(row_pigeon_id), Some(reported_at)) = (row_pigeon_id, reported_at) else {
      console_error!("Greptime SQL row missing pigeon_id/greptime_timestamp, skipping");
      continue;
    };

    for (col, val) in columns.iter().zip(row.iter()) {
      if col == "pigeon_id" || col == "greptime_timestamp" || val.is_null() {
        continue;
      }

      let (value, value_num) = match val {
        serde_json::Value::Number(n) => (n.to_string(), n.as_f64()),
        serde_json::Value::String(s) => {
          let num = s.parse::<f64>().ok();
          (s.clone(), num)
        }
        other => (other.to_string(), None),
      };

      points.push(TelemetryHistoryPoint {
        pigeon_id: row_pigeon_id.clone(),
        key: col.clone(),
        value,
        value_num,
        reported_at,
      });
    }
  }

  points.sort_by_key(|p| p.reported_at);
  Ok(points)
}

/// Backs `GET /pigeons/:id/telemetry/history` when `GREPTIMEDB_ENDPOINT`
/// is configured (task #26) -- the Greptime-first counterpart to
/// `helpers::query_telemetry_history_for_pigeon` (Postgres), which the
/// gateway route falls back to on `Err` here (or skips this entirely if
/// unconfigured). Caller is responsible for ACL-gating before this runs,
/// same convention as the Postgres version.
pub async fn query_greptime_history_for_pigeon(
  env: &Env,
  pigeon_id: &str,
  key: Option<&str>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<Vec<TelemetryHistoryPoint>> {
  query_greptime_history_for_pigeons(env, std::slice::from_ref(&pigeon_id.to_string()), key, since, until).await
}

/// Backs `GET /flocks/:id/telemetry/history` when `GREPTIMEDB_ENDPOINT` is
/// configured -- takes an explicit, already-ownership-checked pigeon-ID
/// list (see `helpers::get_flock_pigeon_ids`, a Postgres round-trip that
/// still has to happen first: Greptime has no `pigeons`/`flocks` tables of
/// its own to resolve flock membership or ownership from -- see the task's
/// design doc, `## 2`, for the full reasoning on why this is a
/// two-round-trip design either way, same shape as the Postgres path's own
/// `JOIN`).
pub async fn query_greptime_history_for_pigeons(
  env: &Env,
  pigeon_ids: &[String],
  key: Option<&str>,
  since: Option<OffsetDateTime>,
  until: Option<OffsetDateTime>,
) -> Result<Vec<TelemetryHistoryPoint>> {
  if pigeon_ids.is_empty() {
    return Ok(Vec::new());
  }

  for id in pigeon_ids {
    if !is_valid_pigeon_id(id) {
      console_error!("Greptime history query: invalid pigeon_id '{id}'");
      return Err(worker::Error::RustError("Bad Request: Invalid pigeon_id".into()));
    }
  }

  let sql = build_history_sql(pigeon_ids, since, until);
  let mut points = query_greptime_sql(env, &sql).await?;

  if let Some(k) = key {
    points.retain(|p| p.key == k);
  }
  points.truncate(5000);

  Ok(points)
}
