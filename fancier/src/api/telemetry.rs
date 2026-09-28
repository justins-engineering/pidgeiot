// A thin wrapper around dovecote's telemetry-history routes and the
// capsules types they share, rather than a parallel guess at either.
// Setting/clearing a pigeon's telemetry endpoint lives in
// api/pigeons.rs (`update_telemetry_endpoint`) alongside the other
// per-pigeon PUT routes, not here.
use crate::api::fetch_json;
use crate::helpers::graph_store::GraphScope;
use capsules::{
  TELEMETRY_HISTORY_TRUNCATED_HEADER, TelemetryHistoryBucket, TelemetryHistoryPoint,
  TelemetryLatest,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;
use wasm_bindgen_futures::JsFuture;
use web_sys::Response;

/// `TelemetryHistoryQuery` (capsules) deserializes `since`/`until` via
/// `time::serde::rfc3339::option`, so the wire format is an RFC3339 string,
/// not a raw unix integer. `:` is the only reserved character an
/// RFC3339-with-`Z`-offset timestamp (always the case for
/// `OffsetDateTime::now_utc()`) ever contains, so a full percent-encoding
/// crate would be overkill — hand-replacing it is enough to keep the query
/// string valid.
fn rfc3339_query_value(t: OffsetDateTime) -> String {
  t.format(&Rfc3339).unwrap_or_default().replace(':', "%3A")
}

/// GET /pigeons/:id/telemetry — latest value per key (mirrors the DO's
/// `pigeon_telemetry` table: one row per key, upserted on each device
/// report — see CLAUDE.md's device-facing telemetry ingestion notes).
pub async fn get_latest(pigeon_id: &str) -> Option<Vec<TelemetryLatest>> {
  let mut path = String::with_capacity(96);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// A history read together with whether the server had to cut the range
/// short. Both facts come from the same response — the points are the
/// body, the cap is `TELEMETRY_HISTORY_TRUNCATED_HEADER` — so callers get
/// them as one value rather than one of them being available only to
/// whoever remembers to look.
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryHistory {
  pub points: Vec<TelemetryHistoryPoint>,
  /// `None` means the response said nothing either way (a backend older
  /// than the header). Deliberately not folded into `false`: "we did not
  /// ask" and "this range is complete" are different claims, and only the
  /// second one is safe to render as a complete window.
  pub truncated: Option<bool>,
}

/// The header is exposed cross-origin by dovecote's `build_cors`
/// (`with_exposed_headers`), so this read works from the browser; without
/// that it would silently always be `None`.
fn truncated_header(response: &Response) -> Option<bool> {
  let raw = response
    .headers()
    .get(TELEMETRY_HISTORY_TRUNCATED_HEADER)
    .ok()
    .flatten()?;
  match raw.trim() {
    "true" => Some(true),
    "false" => Some(false),
    _ => None,
  }
}

/// Shared by both history routes below — identical response handling, only
/// the path differs.
async fn fetch_history(path: &str) -> Option<TelemetryHistory> {
  let response = fetch_json("GET", path, None).await?;
  let truncated = truncated_header(&response);
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  Some(TelemetryHistory {
    points: serde_wasm_bindgen::from_value(json).ok()?,
    truncated,
  })
}

/// Raw shape (`raw=true`): flat, truncating at
/// `capsules::TELEMETRY_HISTORY_MAX_POINTS`. dovecote buckets by default
/// now (see `get_history_buckets` below) -- this stays explicitly raw for
/// the two callers that need real per-report values rather than a
/// bucket's aggregate: `gps_track::gps_fixes_from_history` pairs
/// `gps_lat`/`gps_lon` from the same report, which a bucket mean can't
/// reconstruct, and `connection_state::latest_seen_by_pigeon` needs the
/// true latest timestamp, not a bucket's start (which can understate
/// "last seen" by up to one bucket width).
pub async fn get_history(
  pigeon_id: &str,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<TelemetryHistory> {
  let mut path = String::with_capacity(176);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/history?raw=true&since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  fetch_history(&path).await
}

/// Same `TelemetryHistoryPoint` shape and `raw=true` rationale as
/// `get_history` — the flock-scoped route spans multiple pigeons, but the
/// row already carries `pigeon_id` unconditionally (capsules doesn't have
/// a separate flock-only variant).
pub async fn get_flock_history(
  flock_id: &uuid::Uuid,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<TelemetryHistory> {
  let mut path = String::with_capacity(176);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());
  path.push_str("/telemetry/history?raw=true&since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  fetch_history(&path).await
}

/// Path suffix and first parameter of a raw page, after `/pigeons/<id>` or `/flocks/<id>`.
const RAW_HISTORY_QUERY: &str = "/telemetry/history?raw=true&since=";
/// The page's inclusive upper bound.
const UNTIL_PARAM: &str = "&until=";
/// Comma-separated key filter; left off to export every key.
const KEYS_PARAM: &str = "&keys=";

/// One raw page of either scope's history for the CSV export: the newest
/// `capsules::TELEMETRY_HISTORY_MAX_POINTS` points of `[since, until]` for `keys`, or for every
/// key when `keys` is `None`. The caller always pins `until`, so no page repeats a statement
/// Hyperdrive may have cached.
pub async fn get_history_page(
  scope: &GraphScope,
  keys: Option<&[String]>,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<TelemetryHistory> {
  let since = rfc3339_query_value(since);
  let until = rfc3339_query_value(until);
  let keys: Vec<String> = keys
    .unwrap_or_default()
    .iter()
    .map(|k| String::from(js_sys::encode_uri_component(k)))
    .collect();
  let keys_len: usize = keys.iter().map(|k| k.len() + 1).sum();
  let mut buf = Uuid::encode_buffer();
  let (prefix, id) = match scope {
    GraphScope::Pigeon(id) => ("/pigeons/", id.as_str()),
    GraphScope::Flock(id) => ("/flocks/", &*id.hyphenated().encode_lower(&mut buf)),
  };

  let mut path = String::with_capacity(
    prefix.len()
      + id.len()
      + RAW_HISTORY_QUERY.len()
      + since.len()
      + UNTIL_PARAM.len()
      + until.len()
      + KEYS_PARAM.len()
      + keys_len,
  );
  path.push_str(prefix);
  path.push_str(id);
  path.push_str(RAW_HISTORY_QUERY);
  path.push_str(&since);
  path.push_str(UNTIL_PARAM);
  path.push_str(&until);
  for (i, key) in keys.iter().enumerate() {
    path.push_str(if i == 0 { KEYS_PARAM } else { "," });
    path.push_str(key);
  }

  fetch_history(&path).await
}

/// Shared by both bucketed history fetches below — no truncation header to
/// read: a bucketed response is bounded by construction (see
/// `capsules::TELEMETRY_HISTORY_BUCKET_TARGET`'s doc comment), so there's
/// nothing for dovecote to flag.
async fn fetch_history_buckets(path: &str) -> Option<Vec<TelemetryHistoryBucket>> {
  let response = fetch_json("GET", path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value(json).ok()
}

/// Bucketed shape (the default, no `raw=true`), which is what
/// `graph_widget`'s line charts read.
pub async fn get_history_buckets(
  pigeon_id: &str,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<Vec<TelemetryHistoryBucket>> {
  let mut path = String::with_capacity(160);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  fetch_history_buckets(&path).await
}

/// Flock-scoped counterpart to `get_history_buckets`.
pub async fn get_flock_history_buckets(
  flock_id: &uuid::Uuid,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> Option<Vec<TelemetryHistoryBucket>> {
  let mut path = String::with_capacity(160);
  path.push_str("/flocks/");
  path.push_str(&flock_id.to_string());
  path.push_str("/telemetry/history?since=");
  path.push_str(&rfc3339_query_value(since));
  path.push_str("&until=");
  path.push_str(&rfc3339_query_value(until));

  fetch_history_buckets(&path).await
}
