//! NIDD over Verizon ThingSpace: dovecote's own model of the `NiddService` callback, the frame
//! codec shared with `~/pigeon`, and the pure rules the uplink and downlink paths apply.
//!
//! The frame layout is a contract with C code, so its constants live here rather than in
//! capsules, and docs/api.md is the authority both sides follow. Everything except `sign_frame`,
//! which runs on WebCrypto, is exercised by the host-target tests.

use capsules::{PigeonShadow, TelemetryBatch, TelemetryReading, TelemetryReportBody};
use futures::future::{Either, select};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::pin::pin;
use std::time::Duration;

/// Device to platform: a telemetry body, exactly as the HTTPS telemetry route takes it.
pub const FRAME_TELEMETRY: u8 = 0x01;
/// Device to platform: a shadow report body, exactly as the HTTPS report route takes it.
pub const FRAME_SHADOW_REPORT: u8 = 0x02;
/// Device to platform: the 16 raw claim-key bytes, sent once per boot.
pub const FRAME_HELLO: u8 = 0x04;
/// Platform to device: `target_version`, `current_version`, raw `target_config`, tag.
pub const FRAME_SHADOW: u8 = 0x81;
/// Platform to device: a status code, a `u32` argument, tag.
pub const FRAME_STATUS: u8 = 0x82;

/// `STATUS` code: the report was stored; the argument is the version stored.
pub const STATUS_STORED: u8 = 0x00;
/// `STATUS` code: the account is paused; the argument is how long to hold billable sends.
pub const STATUS_PAUSED: u8 = 0x01;
/// `STATUS` code: the pigeon is not claimed; the argument is 1 when answering a failed `HELLO`.
pub const STATUS_UNCLAIMED: u8 = 0x02;

/// The callback's request id, from the gateway to the pigeon's Durable Object.
pub const HEADER_REQUEST_ID: &str = "X-Nidd-Request-Id";
/// ThingSpace's `callbackCount`: which delivery attempt this is.
pub const HEADER_ATTEMPT: &str = "X-Nidd-Attempt";
/// `paused` when the gateway's free-tier fuse refused a billable frame, else `open`.
pub const HEADER_INGEST: &str = "X-Nidd-Ingest";
/// The line the uplink came from, when the callback named one.
pub const HEADER_LINE: &str = "X-Nidd-Line";

/// Bytes of HMAC-SHA256 that end every platform frame.
pub const NIDD_TAG_BYTES: usize = 8;
/// Bytes in a claim key, which a `HELLO` carries raw.
pub const NIDD_CLAIM_KEY_BYTES: usize = 16;
/// Largest callback body the route parses: over three times the largest legitimate one.
pub const NIDD_CALLBACK_MAX_BYTES: usize = 8192;
/// How long a `PAUSED` notice asks the device to hold its billable sends.
pub const NIDD_PAUSED_HOLD_SECS: u32 = 3600;
/// `maximumDeliveryTime` of every downlink; a push that lapses is re-sent on the next uplink.
pub const NIDD_MT_DELIVERY_SECS: i64 = 86_400;
/// Shortest gap between two unsolicited shadow pushes to one pigeon.
const NIDD_PUSH_HOLD_SECS: i64 = 900;
/// Shortest gap between two `PAUSED` or `UNCLAIMED` notices to one pigeon.
const NIDD_NOTICE_HOLD_SECS: i64 = 3600;
/// De-duplication keys a pigeon remembers, oldest dropped first.
const NIDD_SEEN_KEYS: usize = 64;
/// ThingSpace resends an unacknowledged callback this many seconds after the attempt before.
const NIDD_RESEND_SECS: i64 = 300;

/// The two fields read before a callback is trusted: the password to check, and the request id
/// that names the callback in a log line even when the rest of the body does not parse.
#[derive(Deserialize)]
pub struct CallbackAuth {
  /// The listener password ThingSpace sends in clear inside every callback.
  #[serde(default)]
  pub password: Option<String>,
  /// ThingSpace's request id.
  #[serde(default, rename = "requestId")]
  pub request_id: Option<String>,
}

/// A ThingSpace `NiddService` callback, holding only what dovecote reads. `username` and
/// `password` are deliberately absent, so a parsed callback never carries the credential.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NiddCallback {
  /// ThingSpace's request id; the same for every callback about one downlink.
  #[serde(default)]
  pub request_id: Option<String>,
  /// The top-level identifiers, which name the device the way the request did.
  #[serde(default)]
  pub device_ids: Vec<CarrierId>,
  /// `Delivered`, `Queued`, `DeliveryFailed`, `ConfigCreated`, or a failure status.
  #[serde(default)]
  pub status: Option<String>,
  /// Which delivery attempt this is, from 1.
  #[serde(default)]
  pub callback_count: Option<i64>,
  /// What the callback reports.
  pub nidd_response: NiddResponse,
}

/// The three documented callback variants. Any other is a shape dovecote does not know, and the
/// callback fails to parse.
#[derive(Deserialize)]
pub enum NiddResponse {
  /// A device's uplink.
  #[serde(rename = "niddMONotificationResponse")]
  Uplink(NiddUplink),
  /// The fate of a downlink.
  #[serde(rename = "niddMTDeliveryResponse")]
  Delivery(NiddReport),
  /// The result of configuring a line for NIDD.
  #[serde(rename = "niddConfigResponse")]
  Config(NiddReport),
}

/// An uplink's inner object.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NiddUplink {
  /// The billing account the line belongs to.
  #[serde(default)]
  pub account_name: Option<String>,
  /// The frame, base64.
  #[serde(default)]
  pub message: Option<String>,
  /// Every identifier the carrier holds for the line.
  #[serde(default)]
  pub device_ids: Vec<CarrierId>,
}

/// A delivery report's or configuration result's inner object.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NiddReport {
  /// The billing account the line belongs to.
  #[serde(default)]
  pub account_name: Option<String>,
  /// Why a delivery or configuration did not succeed, from a fixed vocabulary.
  #[serde(default)]
  pub reason: Option<String>,
  /// Every identifier the carrier holds for the line.
  #[serde(default)]
  pub device_ids: Vec<CarrierId>,
}

/// One carrier identifier. Verizon's inner lists carry entries without an id.
#[derive(Deserialize)]
pub struct CarrierId {
  /// The identifier's value.
  #[serde(default)]
  pub id: Option<String>,
  /// `IMEI`, `ICCID`, `IMSI`, `MDN` and so on, in no fixed case.
  #[serde(default)]
  pub kind: String,
}

impl NiddCallback {
  /// The variant's own identifier list.
  fn inner_ids(&self) -> &[CarrierId] {
    match &self.nidd_response {
      NiddResponse::Uplink(uplink) => &uplink.device_ids,
      NiddResponse::Delivery(report) | NiddResponse::Config(report) => &report.device_ids,
    }
  }

  /// The billing account the inner object names.
  pub fn account_name(&self) -> Option<&str> {
    match &self.nidd_response {
      NiddResponse::Uplink(uplink) => uplink.account_name.as_deref(),
      NiddResponse::Delivery(report) | NiddResponse::Config(report) => {
        report.account_name.as_deref()
      }
    }
  }

  /// The variant's name for log lines.
  pub fn kind(&self) -> &'static str {
    match &self.nidd_response {
      NiddResponse::Uplink(_) => "uplink",
      NiddResponse::Delivery(_) => "delivery",
      NiddResponse::Config(_) => "config",
    }
  }
}

/// The 15-digit IMEI a pigeon is keyed by, from a carrier's spelling of it: 15 digits whose Luhn
/// check passes, 14 digits given their check digit, or a 16-digit IMEISV cut to 14 and given it.
/// Anything else is `None`.
pub fn imei_key(raw: &str) -> Option<String> {
  let raw = raw.trim();
  if !raw.bytes().all(|b| b.is_ascii_digit()) {
    return None;
  }
  match raw.len() {
    15 => capsules::imei_is_valid(raw).then(|| raw.to_string()),
    14 | 16 => {
      let body = &raw[..14];
      let mut key = String::with_capacity(15);
      key.push_str(body);
      key.push(luhn_digit(body));
      Some(key)
    }
    _ => None,
  }
}

/// The Luhn check digit for a string of ASCII digits.
fn luhn_digit(body: &str) -> char {
  let sum: u32 = body
    .bytes()
    .rev()
    .enumerate()
    .map(|(i, b)| {
      let d = u32::from(b - b'0');
      match i % 2 {
        0 if d * 2 > 9 => d * 2 - 9,
        0 => d * 2,
        _ => d,
      }
    })
    .sum();
  // The digit is always 0..=9.
  char::from(b'0' + ((10 - sum % 10) % 10) as u8)
}

/// The IMEI a callback is about: the first `imei` entry that normalizes, from the variant's own
/// list first and the top-level list second, `kind` compared case-insensitively.
pub fn callback_imei(callback: &NiddCallback) -> Option<String> {
  callback
    .inner_ids()
    .iter()
    .chain(&callback.device_ids)
    .filter(|entry| entry.kind.eq_ignore_ascii_case("imei"))
    .find_map(|entry| entry.id.as_deref().and_then(imei_key))
}

/// The line an uplink came from, for the claim's line pin: the ICCID of the uplink's own list,
/// else its IMSI. Only an id of ASCII letters and digits counts, since it travels as a header;
/// anything else reads as no line at all, which a pinned pigeon refuses.
pub fn callback_line(callback: &NiddCallback) -> Option<String> {
  let NiddResponse::Uplink(uplink) = &callback.nidd_response else {
    return None;
  };
  ["iccid", "imsi"].iter().find_map(|kind| {
    uplink
      .device_ids
      .iter()
      .filter(|entry| entry.kind.eq_ignore_ascii_case(kind))
      .find_map(|entry| entry.id.as_deref())
      .filter(|id| {
        !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())
      })
      .map(str::to_string)
  })
}

/// The name a Nidd pigeon's Durable Object id derives from, so a callback reaches it by IMEI.
pub fn nidd_object_name(imei: &str) -> String {
  let mut name = String::with_capacity(10 + imei.len());
  name.push_str("nidd:imei:");
  name.push_str(imei);
  name
}

/// A parse failure as a log line: the context, the error's category and its column. Never the
/// error's `Display`, which quotes the offending value, an IMEI sent as a number included.
pub fn parse_error_line(context: &str, error: &serde_json::Error) -> String {
  let category = match error.classify() {
    serde_json::error::Category::Io => "io",
    serde_json::error::Category::Syntax => "syntax",
    serde_json::error::Category::Data => "data",
    serde_json::error::Category::Eof => "eof",
  };
  let column = error.column().to_string();
  let mut line = String::with_capacity(context.len() + 17 + category.len() + column.len());
  line.push_str(context);
  line.push_str(" category=");
  line.push_str(category);
  line.push_str(" column=");
  line.push_str(&column);
  line
}

/// An uplink frame, split by its type byte.
#[derive(Debug, PartialEq)]
pub enum Uplink<'a> {
  /// A telemetry body.
  Telemetry(&'a [u8]),
  /// A shadow report body.
  ShadowReport(&'a [u8]),
  /// A `HELLO`'s body, meant to be the 16 claim-key bytes.
  Hello(&'a [u8]),
  /// A type this build does not know, including the reserved log upload.
  Unknown(u8),
  /// No bytes at all.
  Empty,
}

/// Splits an uplink frame into its type and body.
pub fn decode_uplink(frame: &[u8]) -> Uplink<'_> {
  match frame.split_first() {
    None => Uplink::Empty,
    Some((&FRAME_TELEMETRY, body)) => Uplink::Telemetry(body),
    Some((&FRAME_SHADOW_REPORT, body)) => Uplink::ShadowReport(body),
    Some((&FRAME_HELLO, body)) => Uplink::Hello(body),
    Some((&other, _)) => Uplink::Unknown(other),
  }
}

/// Whether a frame of this type is billed and so paused with the account.
pub fn is_billable(frame_type: u8) -> bool {
  matches!(frame_type, FRAME_TELEMETRY | FRAME_SHADOW_REPORT)
}

/// An unsigned `SHADOW` frame: the two versions little-endian, then the raw `target_config`.
pub fn shadow_frame(shadow: &PigeonShadow) -> Vec<u8> {
  let config = shadow.target_config.clone().into_inner();
  let mut frame = Vec::with_capacity(9 + config.len() + NIDD_TAG_BYTES);
  frame.push(FRAME_SHADOW);
  frame.extend_from_slice(&shadow.target_version.to_le_bytes());
  frame.extend_from_slice(&shadow.current_version.to_le_bytes());
  frame.extend_from_slice(config.as_bytes());
  frame
}

/// An unsigned `STATUS` frame.
pub fn status_frame(code: u8, arg: u32) -> Vec<u8> {
  let mut frame = Vec::with_capacity(6 + NIDD_TAG_BYTES);
  frame.push(FRAME_STATUS);
  frame.push(code);
  frame.extend_from_slice(&arg.to_le_bytes());
  frame
}

/// Appends the frame's tag: the first 8 bytes of HMAC-SHA256 over the frame, keyed by the claim
/// key. The device drops a platform frame whose tag does not verify.
pub async fn sign_frame(
  key: &[u8; NIDD_CLAIM_KEY_BYTES],
  mut frame: Vec<u8>,
) -> Result<Vec<u8>, String> {
  let mac = super::stripe_webhook::hmac_sha256(key, &frame).await?;
  let Some(tag) = mac.get(..NIDD_TAG_BYTES) else {
    return Err("HMAC shorter than a frame tag".into());
  };
  frame.extend_from_slice(tag);
  Ok(frame)
}

/// A claim key's bytes from its stored 32-character hex form.
pub fn claim_key_bytes(hex: &str) -> Option<[u8; NIDD_CLAIM_KEY_BYTES]> {
  let digits = hex.as_bytes();
  if digits.len() != NIDD_CLAIM_KEY_BYTES * 2 {
    return None;
  }
  let mut key = [0u8; NIDD_CLAIM_KEY_BYTES];
  for (byte, pair) in key.iter_mut().zip(digits.chunks_exact(2)) {
    *byte = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
  }
  Some(key)
}

fn hex_value(digit: u8) -> Option<u8> {
  match digit {
    b'0'..=b'9' => Some(digit - b'0'),
    b'a'..=b'f' => Some(digit - b'a' + 10),
    b'A'..=b'F' => Some(digit - b'A' + 10),
    _ => None,
  }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The de-duplication key of one uplink: the request id and the first 16 hex characters of the
/// frame's SHA-256. A ThingSpace resend repeats both halves.
pub fn dedupe_key(request_id: &str, frame: &[u8]) -> String {
  let digest = Sha256::digest(frame);
  let mut key = String::with_capacity(request_id.len() + 17);
  key.push_str(request_id);
  key.push(':');
  for byte in &digest[..8] {
    key.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
    key.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
  }
  key
}

/// `pigeon_nidd` as SQL returns it, `seen` still JSON text.
#[derive(Deserialize)]
pub struct NiddSqlRow {
  claimed_at: Option<i64>,
  line_id: Option<String>,
  awaiting_version: i32,
  pushed_version: i32,
  pushed_at: i64,
  notice_at: i64,
  seen: String,
}

/// A pigeon's NIDD state: its claim, the push it owes the device, and the uplinks it has seen.
/// The default is an unclaimed pigeon with nothing outstanding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NiddRow {
  /// When the device last claimed the pigeon; `None` while unclaimed.
  pub claimed_at: Option<i64>,
  /// The ICCID or IMSI the claim is pinned to; `None` when the claiming callback named neither.
  pub line_id: Option<String>,
  /// The newest `target_version` the device has not confirmed; 0 when converged.
  pub awaiting_version: i32,
  /// The `target_version` of the last `SHADOW` sent; 0 after a failed send.
  pub pushed_version: i32,
  /// When that `SHADOW` was sent; 0 after a failed send.
  pub pushed_at: i64,
  /// When the last `PAUSED` or `UNCLAIMED` notice went out.
  pub notice_at: i64,
  /// The last de-duplication keys, oldest first.
  pub seen: Vec<String>,
}

impl From<NiddSqlRow> for NiddRow {
  /// An unreadable `seen` reads as empty: the worst it costs is one repeat stored twice.
  fn from(row: NiddSqlRow) -> Self {
    Self {
      claimed_at: row.claimed_at,
      line_id: row.line_id,
      awaiting_version: row.awaiting_version,
      pushed_version: row.pushed_version,
      pushed_at: row.pushed_at,
      notice_at: row.notice_at,
      seen: serde_json::from_str(&row.seen).unwrap_or_default(),
    }
  }
}

impl NiddRow {
  /// Whether this uplink has been stored already.
  pub fn has_seen(&self, key: &str) -> bool {
    self.seen.iter().any(|seen| seen == key)
  }

  /// Records an uplink's key, forgetting the oldest beyond the window.
  pub fn remember(&mut self, key: String) {
    self.seen.push(key);
    if self.seen.len() > NIDD_SEEN_KEYS {
      let excess = self.seen.len() - NIDD_SEEN_KEYS;
      self.seen.drain(..excess);
    }
  }

  /// `seen` as the JSON text the table stores.
  pub fn seen_json(&self) -> String {
    serde_json::to_string(&self.seen).unwrap_or_else(|_| "[]".to_string())
  }
}

/// Whether a `PAUSED` or `UNCLAIMED` notice may go out: at most one an hour per pigeon.
pub fn notice_due(row: &NiddRow, now: i64) -> bool {
  now - row.notice_at >= NIDD_NOTICE_HOLD_SECS
}

/// Whether an unsolicited shadow push is due: the device is claimed and behind, and either the
/// newest target has not been sent and no push went out in the last hold window, or the last
/// push's delivery window has passed without the device confirming it.
pub fn shadow_push_due(row: &NiddRow, now: i64) -> bool {
  row.claimed_at.is_some()
    && row.awaiting_version != 0
    && ((row.pushed_version < row.awaiting_version && now - row.pushed_at >= NIDD_PUSH_HOLD_SECS)
      || now - row.pushed_at > NIDD_MT_DELIVERY_SECS)
}

/// Runs `future` until `limit` passes: `None` when the timer won. The future is dropped rather
/// than aborted, so a request it already sent may still land.
pub async fn within<F: Future>(limit: Duration, future: F) -> Option<F::Output> {
  let future = pin!(future);
  let deadline = pin!(worker::Delay::from(limit));
  match select(future, deadline).await {
    Either::Left((output, _)) => Some(output),
    Either::Right(((), _)) => None,
  }
}

/// How much older than its arrival a reading first stored on this delivery attempt really is:
/// each earlier attempt came five minutes before the next.
pub fn resend_age_secs(attempt: i64) -> i64 {
  (attempt - 1).clamp(0, 3) * NIDD_RESEND_SECS
}

/// Ages a telemetry body by `extra_secs`. A flat map becomes one reading of that age, and each
/// batch reading's age grows by it; a reading that carries only `at` is absolute and left alone.
/// The existing 24-hour clamp applies downstream.
pub fn backdate(body: TelemetryReportBody, extra_secs: i64) -> TelemetryReportBody {
  if extra_secs <= 0 {
    return body;
  }
  match body {
    TelemetryReportBody::Flat(metrics) => TelemetryReportBody::Batch(TelemetryBatch {
      reports: vec![TelemetryReading {
        at: None,
        age_secs: Some(extra_secs),
        metrics,
      }],
    }),
    TelemetryReportBody::Batch(mut batch) => {
      for reading in &mut batch.reports {
        match (reading.age_secs, reading.at) {
          (Some(age), _) => reading.age_secs = Some(age.max(0).saturating_add(extra_secs)),
          (None, Some(_)) => {}
          (None, None) => reading.age_secs = Some(extra_secs),
        }
      }
      TelemetryReportBody::Batch(batch)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::JsonString;
  use std::collections::HashMap;

  const IMEI: &str = "490154203237518";

  /// Verizon's documented MO callback, with a Luhn-valid IMEI and placeholder credentials.
  fn uplink_body() -> String {
    r#"{"username":"user","password":"pwd","requestId":"a0fff7d6-6b30-45eb-84d7-0bc103d319c0",
      "deviceIds":[{"id":"490154203237518","kind":"IMEI"}],
      "niddResponse":{"niddMONotificationResponse":{"accountName":"9999080353-00001",
        "message":"QUJD","deviceIds":[{"id":"490154203237518","kind":"IMEI"},
        {"id":"311480852590999","kind":"IMSI"},{"id":"9998501090","kind":"MDN"},
        {"id":"99948000004347398194","kind":"ICCID"}]}},
      "callbackCount":1,"maxCallbackThreshold":4}"#
      .to_string()
  }

  fn delivery_body(status: &str, reason: Option<&str>) -> String {
    let reason = reason.map_or(String::new(), |r| format!(r#""reason":"{r}","#));
    format!(
      r#"{{"username":"user","password":"pwd","requestId":"aeb0ed2f",
        "deviceIds":[{{"id":"490154203237518","kind":"IMEI"}}],
        "niddResponse":{{"niddMTDeliveryResponse":{{"accountName":"9999080353-00001",{reason}
          "deviceIds":[{{"id":"490154203237518","kind":"Imei"}},
          {{"id":"99948000005795977263","kind":"ICCID"}}]}}}},
        "status":"{status}","callbackCount":1,"maxCallbackThreshold":4}}"#
    )
  }

  fn config_body(status: &str, reason: Option<&str>) -> String {
    let reason = reason.map_or(String::new(), |r| format!(r#""reason":"{r}","#));
    format!(
      r#"{{"username":"user name","password":"password","requestId":"595f5c44",
        "niddResponse":{{"niddConfigResponse":{{"accountName":"9992330389-00001",{reason}
          "deviceIds":[{{"id":"99962019000000000000000000000002","kind":"EID"}},
          {{"id":"490154203237518","kind":"imei"}}]}}}},
        "status":"{status}","callbackCount":1,"maxCallbackThreshold":4}}"#
    )
  }

  fn parse(body: &str) -> NiddCallback {
    serde_json::from_str(body).expect("documented callback parses")
  }

  fn id(kind: &str, value: &str) -> CarrierId {
    CarrierId {
      id: Some(value.to_string()),
      kind: kind.to_string(),
    }
  }

  fn uplink_with(inner: Vec<CarrierId>, top: Vec<CarrierId>) -> NiddCallback {
    NiddCallback {
      request_id: None,
      device_ids: top,
      status: None,
      callback_count: None,
      nidd_response: NiddResponse::Uplink(NiddUplink {
        account_name: None,
        message: None,
        device_ids: inner,
      }),
    }
  }

  #[test]
  fn the_documented_uplink_parses() {
    let callback = parse(&uplink_body());
    assert_eq!(callback.kind(), "uplink");
    assert_eq!(callback.account_name(), Some("9999080353-00001"));
    assert_eq!(callback.callback_count, Some(1));
    assert_eq!(callback_imei(&callback).as_deref(), Some(IMEI));
    let NiddResponse::Uplink(uplink) = &callback.nidd_response else {
      panic!("uplink parsed as another variant");
    };
    assert_eq!(uplink.message.as_deref(), Some("QUJD"));
  }

  #[test]
  fn the_documented_delivery_reports_parse() {
    for (status, reason) in [
      ("Delivered", None),
      ("Queued", Some("Buffered, device not reachable")),
      ("DeliveryFailed", Some("unknown")),
    ] {
      let callback = parse(&delivery_body(status, reason));
      assert_eq!(callback.kind(), "delivery");
      assert_eq!(callback.status.as_deref(), Some(status));
      let NiddResponse::Delivery(report) = &callback.nidd_response else {
        panic!("delivery report parsed as another variant");
      };
      assert_eq!(report.reason.as_deref(), reason);
      assert_eq!(callback_imei(&callback).as_deref(), Some(IMEI));
      assert_eq!(callback_line(&callback), None);
    }
  }

  #[test]
  fn the_documented_configuration_results_parse() {
    let created = parse(&config_body("ConfigCreated", None));
    assert_eq!(created.kind(), "config");
    assert_eq!(created.status.as_deref(), Some("ConfigCreated"));
    assert!(created.device_ids.is_empty());
    assert_eq!(callback_imei(&created).as_deref(), Some(IMEI));

    let failed = parse(&config_body(
      "ConfigFailed",
      Some("Not CatM or NBIoT Device"),
    ));
    let NiddResponse::Config(report) = &failed.nidd_response else {
      panic!("configuration result parsed as another variant");
    };
    assert_eq!(report.reason.as_deref(), Some("Not CatM or NBIoT Device"));
  }

  #[test]
  fn a_callback_without_top_level_ids_or_a_count_parses() {
    let body = r#"{"requestId":"r","niddResponse":{"niddMONotificationResponse":
      {"accountName":"a","message":"AQ==","deviceIds":[{"kind":"IMSI"}]}}}"#;
    let callback = parse(body);
    assert!(callback.device_ids.is_empty());
    assert_eq!(callback.callback_count, None);
    assert_eq!(callback_imei(&callback), None);
  }

  #[test]
  fn an_unknown_variant_does_not_parse() {
    let body = r#"{"requestId":"r","niddResponse":{"niddSomethingElse":{"accountName":"a"}}}"#;
    assert!(serde_json::from_str::<NiddCallback>(body).is_err());
  }

  #[test]
  fn callback_auth_reads_the_request_id() {
    let auth: CallbackAuth = serde_json::from_str(&uplink_body()).unwrap();
    assert_eq!(auth.password.as_deref(), Some("pwd"));
    assert_eq!(
      auth.request_id.as_deref(),
      Some("a0fff7d6-6b30-45eb-84d7-0bc103d319c0")
    );

    let bare: CallbackAuth = serde_json::from_str("{}").unwrap();
    assert!(bare.password.is_none() && bare.request_id.is_none());
  }

  #[test]
  fn a_parse_error_line_never_quotes_the_value() {
    let body = r#"{"requestId":"r","deviceIds":[{"id":490154203237518,"kind":"IMEI"}],
      "niddResponse":{"niddConfigResponse":{}}}"#;
    let Err(error) = serde_json::from_str::<NiddCallback>(body) else {
      panic!("a numeric IMEI parsed");
    };
    // The Display is what must stay out of the logs.
    assert!(error.to_string().contains("490154203237518"));

    let line = parse_error_line("nidd_cb parse=callback", &error);
    assert!(line.starts_with("nidd_cb parse=callback category=data column="));
    assert!(!line.contains("4901542"));
    assert!(!line.contains("37518"));
  }

  #[test]
  fn imei_key_normalizes_the_three_lengths() {
    assert_eq!(imei_key(IMEI).as_deref(), Some(IMEI));
    assert_eq!(imei_key(" 490154203237518 ").as_deref(), Some(IMEI));
    assert_eq!(imei_key("49015420323751").as_deref(), Some(IMEI));
    assert_eq!(imei_key("4901542032375107").as_deref(), Some(IMEI));
  }

  #[test]
  fn imei_key_refuses_a_bad_check_digit_and_junk() {
    assert_eq!(imei_key("490154203237519"), None);
    assert_eq!(imei_key("4901542032375"), None);
    assert_eq!(imei_key("49015420323751800"), None);
    assert_eq!(imei_key("49015420323751a"), None);
    assert_eq!(imei_key("49-015420-323751-8"), None);
    assert_eq!(imei_key(""), None);
  }

  #[test]
  fn callback_imei_takes_any_case_inner_list_first() {
    for kind in ["IMEI", "imei", "Imei"] {
      let inner = uplink_with(vec![id(kind, IMEI)], vec![]);
      assert_eq!(callback_imei(&inner).as_deref(), Some(IMEI));
      let top = uplink_with(vec![], vec![id(kind, IMEI)]);
      assert_eq!(callback_imei(&top).as_deref(), Some(IMEI));
    }

    let other = "353456789012348";
    assert!(capsules::imei_is_valid(other));
    let both = uplink_with(vec![id("IMEI", IMEI)], vec![id("IMEI", other)]);
    assert_eq!(callback_imei(&both).as_deref(), Some(IMEI));

    let unusable_inner = uplink_with(vec![id("IMEI", "junk")], vec![id("IMEI", other)]);
    assert_eq!(callback_imei(&unusable_inner).as_deref(), Some(other));

    let none = uplink_with(vec![id("MDN", "9998501090")], vec![]);
    assert_eq!(callback_imei(&none), None);
  }

  #[test]
  fn callback_line_takes_the_iccid_before_the_imsi_from_the_inner_list() {
    let both = uplink_with(
      vec![
        id("imsi", "311480852590999"),
        id("Iccid", "89148000004347398194"),
      ],
      vec![],
    );
    assert_eq!(
      callback_line(&both).as_deref(),
      Some("89148000004347398194")
    );

    let imsi_only = uplink_with(vec![id("IMSI", "311480852590999")], vec![]);
    assert_eq!(
      callback_line(&imsi_only).as_deref(),
      Some("311480852590999")
    );

    let top_only = uplink_with(vec![], vec![id("ICCID", "89148000004347398194")]);
    assert_eq!(callback_line(&top_only), None);

    let unusable = uplink_with(vec![id("ICCID", "8914\r\nX")], vec![]);
    assert_eq!(callback_line(&unusable), None);
  }

  #[test]
  fn the_object_name_carries_the_imei() {
    assert_eq!(nidd_object_name(IMEI), "nidd:imei:490154203237518");
  }

  #[test]
  fn every_uplink_type_decodes() {
    assert_eq!(decode_uplink(&[]), Uplink::Empty);
    assert_eq!(decode_uplink(&[0x01, b'{', b'}']), Uplink::Telemetry(b"{}"));
    assert_eq!(decode_uplink(&[0x02, b'{']), Uplink::ShadowReport(b"{"));
    assert_eq!(decode_uplink(&[0x04, 1, 2]), Uplink::Hello(&[1, 2]));
    assert_eq!(decode_uplink(&[0x03, 9]), Uplink::Unknown(0x03));
    assert_eq!(decode_uplink(&[0x81]), Uplink::Unknown(0x81));
    assert!(is_billable(FRAME_TELEMETRY) && is_billable(FRAME_SHADOW_REPORT));
    assert!(!is_billable(FRAME_HELLO) && !is_billable(0x03));
  }

  #[test]
  fn the_shadow_frame_matches_the_documented_bytes() {
    let shadow = PigeonShadow {
      target_version: 8,
      current_version: 7,
      target_config: JsonString::new(r#"{"telemetry_interval":900,"log":true}"#.to_string())
        .unwrap(),
      ..Default::default()
    };
    let mut expected = vec![0x81, 0x08, 0, 0, 0, 0x07, 0, 0, 0];
    expected.extend_from_slice(br#"{"telemetry_interval":900,"log":true}"#);
    let frame = shadow_frame(&shadow);
    assert_eq!(frame.len(), 46);
    assert_eq!(frame, expected);
  }

  #[test]
  fn the_status_frames_match_the_documented_bytes() {
    assert_eq!(status_frame(STATUS_STORED, 7), [0x82, 0x00, 0x07, 0, 0, 0]);
    assert_eq!(
      status_frame(STATUS_PAUSED, NIDD_PAUSED_HOLD_SECS),
      [0x82, 0x01, 0x10, 0x0e, 0, 0]
    );
    assert_eq!(status_frame(STATUS_UNCLAIMED, 0), [0x82, 0x02, 0, 0, 0, 0]);
    assert_eq!(
      status_frame(STATUS_UNCLAIMED, 1),
      [0x82, 0x02, 0x01, 0, 0, 0]
    );
  }

  #[test]
  fn a_claim_key_decodes_from_hex() {
    let key = claim_key_bytes("00112233445566778899aabbccddeeff").unwrap();
    assert_eq!(key[0], 0x00);
    assert_eq!(key[9], 0x99);
    assert_eq!(key[15], 0xff);
    assert_eq!(
      claim_key_bytes("00112233445566778899AABBCCDDEEFF"),
      Some(key)
    );
    assert_eq!(claim_key_bytes("00112233445566778899aabbccddeef"), None);
    assert_eq!(claim_key_bytes("00112233445566778899aabbccddeeg0"), None);
  }

  #[test]
  fn the_dedupe_key_is_the_request_id_and_a_digest_prefix() {
    // SHA-256("ABC") begins b5d4045c3f466fa9.
    assert_eq!(dedupe_key("req", b"ABC"), "req:b5d4045c3f466fa9");
    assert_ne!(dedupe_key("req", b"ABD"), dedupe_key("req", b"ABC"));
    assert_ne!(dedupe_key("other", b"ABC"), dedupe_key("req", b"ABC"));
  }

  #[test]
  fn remember_keeps_the_newest_sixty_four() {
    let mut row = NiddRow::default();
    for i in 0..70 {
      row.remember(i.to_string());
    }
    assert_eq!(row.seen.len(), 64);
    assert_eq!(row.seen.first().map(String::as_str), Some("6"));
    assert!(row.has_seen("69") && !row.has_seen("5"));

    let stored: NiddRow = NiddSqlRow {
      claimed_at: None,
      line_id: None,
      awaiting_version: 0,
      pushed_version: 0,
      pushed_at: 0,
      notice_at: 0,
      seen: row.seen_json(),
    }
    .into();
    assert_eq!(stored.seen, row.seen);
  }

  #[test]
  fn an_unreadable_seen_list_reads_as_empty() {
    let row: NiddRow = NiddSqlRow {
      claimed_at: Some(1),
      line_id: None,
      awaiting_version: 0,
      pushed_version: 0,
      pushed_at: 0,
      notice_at: 0,
      seen: "not json".to_string(),
    }
    .into();
    assert!(row.seen.is_empty());
    assert_eq!(row.claimed_at, Some(1));
  }

  #[test]
  fn a_notice_goes_out_at_most_hourly() {
    let row = NiddRow {
      notice_at: 10_000,
      ..Default::default()
    };
    assert!(!notice_due(&row, 10_000));
    assert!(!notice_due(&row, 13_599));
    assert!(notice_due(&row, 13_600));
    assert!(notice_due(&NiddRow::default(), 10_000));
  }

  #[test]
  fn shadow_push_due_truth_table() {
    let now = 1_000_000;
    let claimed = NiddRow {
      claimed_at: Some(1),
      awaiting_version: 8,
      ..Default::default()
    };

    // Never pushed: due.
    assert!(shadow_push_due(&claimed, now));

    // Hold: a push of an older version went out ten minutes ago.
    let held = NiddRow {
      pushed_version: 7,
      pushed_at: now - 600,
      ..claimed.clone()
    };
    assert!(!shadow_push_due(&held, now));
    assert!(shadow_push_due(&held, now - 600 + 900));

    // The newest version is already out and inside its delivery window.
    let outstanding = NiddRow {
      pushed_version: 8,
      pushed_at: now - 3600,
      ..claimed.clone()
    };
    assert!(!shadow_push_due(&outstanding, now));

    // Lapse: that push's delivery window has passed unconfirmed.
    let lapsed = NiddRow {
      pushed_at: now - 86_401,
      ..outstanding.clone()
    };
    assert!(shadow_push_due(&lapsed, now));

    // Failed send: the reset makes it due again.
    let failed = NiddRow {
      pushed_version: 0,
      pushed_at: 0,
      ..claimed.clone()
    };
    assert!(shadow_push_due(&failed, now));

    // Unclaimed, or converged: never.
    let unclaimed = NiddRow {
      claimed_at: None,
      ..claimed.clone()
    };
    assert!(!shadow_push_due(&unclaimed, now));
    let converged = NiddRow {
      awaiting_version: 0,
      ..lapsed
    };
    assert!(!shadow_push_due(&converged, now));
  }

  #[test]
  fn resend_age_counts_earlier_attempts() {
    assert_eq!(resend_age_secs(0), 0);
    assert_eq!(resend_age_secs(1), 0);
    assert_eq!(resend_age_secs(2), 300);
    assert_eq!(resend_age_secs(4), 900);
    assert_eq!(resend_age_secs(9), 900);
  }

  fn metrics() -> HashMap<String, String> {
    HashMap::from([("uptime_s".to_string(), "85800".to_string())])
  }

  fn reading(at: Option<i64>, age_secs: Option<i64>) -> TelemetryReading {
    TelemetryReading {
      at,
      age_secs,
      metrics: metrics(),
    }
  }

  #[test]
  fn backdate_ages_a_flat_map_into_one_reading() {
    let aged = backdate(TelemetryReportBody::Flat(metrics()), 600);
    assert_eq!(
      aged,
      TelemetryReportBody::Batch(TelemetryBatch {
        reports: vec![reading(None, Some(600))],
      })
    );
  }

  #[test]
  fn backdate_shifts_a_batch_and_leaves_absolute_readings_alone() {
    let batch = TelemetryReportBody::Batch(TelemetryBatch {
      reports: vec![
        reading(None, Some(300)),
        reading(Some(1_700_000_000), None),
        reading(None, None),
        reading(Some(1_700_000_000), Some(0)),
      ],
    });
    assert_eq!(
      backdate(batch, 300),
      TelemetryReportBody::Batch(TelemetryBatch {
        reports: vec![
          reading(None, Some(600)),
          reading(Some(1_700_000_000), None),
          reading(None, Some(300)),
          reading(Some(1_700_000_000), Some(300)),
        ],
      })
    );
  }

  #[test]
  fn backdate_on_a_first_attempt_changes_nothing() {
    let flat = TelemetryReportBody::Flat(metrics());
    assert_eq!(backdate(flat.clone(), resend_age_secs(1)), flat);
  }
}
