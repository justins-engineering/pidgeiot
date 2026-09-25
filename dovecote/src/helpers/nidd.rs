//! NIDD over Verizon ThingSpace: dovecote's own model of the `NiddService` callback, the frame
//! codec shared with `~/pigeon`, and the pure rules the uplink and downlink paths apply.
//!
//! The frame layout is a contract with C code, so its constants live here rather than in
//! capsules, and docs/api.md is the authority both sides follow. Everything except `sign_frame`,
//! which runs on WebCrypto, is exercised by the host-target tests.

use super::constant_time_eq;
use capsules::{NIDD_MAX_FRAME_BYTES, PigeonShadow};
use futures::future::{Either, select};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::pin::pin;
use std::time::Duration;

/// Device to platform: a telemetry body, exactly as the HTTPS telemetry route takes it.
pub const FRAME_TELEMETRY: u8 = 0x01;
/// Device to platform: a shadow report body, exactly as the HTTPS report route takes it.
pub const FRAME_SHADOW_REPORT: u8 = 0x02;
/// Device to platform: the claim key as 32 lowercase hex characters, sent once per boot.
pub const FRAME_HELLO: u8 = 0x04;
/// Platform to device: a header of `target_version` and `current_version`, the raw
/// `target_config`, then the tag.
pub const FRAME_SHADOW: u8 = 0x81;
/// Platform to device: a header of the status code and its argument, then the tag.
pub const FRAME_STATUS: u8 = 0x82;

/// `STATUS` code: the report was stored; the argument is the version stored.
pub const STATUS_STORED: u8 = 0x00;
/// `STATUS` code: the account is paused; the argument is how long to hold billable sends.
pub const STATUS_PAUSED: u8 = 0x01;
/// `STATUS` code: the pigeon is not claimed; the argument is 1 when answering a failed `HELLO`.
pub const STATUS_UNCLAIMED: u8 = 0x02;

/// The callback's request id, from the gateway to the pigeon's Durable Object.
pub const HEADER_REQUEST_ID: &str = "X-Nidd-Request-Id";
/// `paused` when the gateway's free-tier fuse refused a billable frame, else `open`.
pub const HEADER_INGEST: &str = "X-Nidd-Ingest";
/// The line the uplink came from, when the callback named one.
pub const HEADER_LINE: &str = "X-Nidd-Line";

/// Lowercase hex characters that end every platform frame: the first 8 bytes of HMAC-SHA256.
pub const NIDD_TAG_CHARS: usize = 16;
/// Bytes in a claim key, which a `HELLO` carries as twice as many hex characters.
pub const NIDD_CLAIM_KEY_BYTES: usize = 16;
/// Longest `SHADOW` header: two ten-digit versions, the space between them and the newline.
const NIDD_SHADOW_HEADER_MAX: usize = 22;
/// Longest `STATUS` header: a three-digit code, a ten-digit argument, the space and the newline.
const NIDD_STATUS_HEADER_MAX: usize = 15;
/// Largest callback body the route parses: over three times the largest legitimate one.
pub const NIDD_CALLBACK_MAX_BYTES: usize = 8192;
/// How long a `PAUSED` notice asks the device to hold its billable sends.
pub const NIDD_PAUSED_HOLD_SECS: u32 = 3600;
/// `maximumDeliveryTime` of every downlink, and how long a sent `SHADOW` is left to land before
/// the next uplink carries it again. A frame that misses the device's connection was never seen
/// to arrive later, so it is not held.
pub const NIDD_MT_DELIVERY_SECS: i64 = 30;
/// Shortest gap between two unsolicited shadow pushes to one pigeon.
const NIDD_PUSH_HOLD_SECS: i64 = 900;
/// Shortest gap between two `PAUSED` or `UNCLAIMED` notices to one pigeon.
const NIDD_NOTICE_HOLD_SECS: i64 = 3600;
/// De-duplication keys a pigeon remembers, oldest dropped first.
const NIDD_SEEN_KEYS: usize = 64;

/// The fields read before a callback is trusted: the password to check, and the request id and
/// attempt that name the callback in a log line, a refused one or one whose body does not parse
/// included, so a refusal can be matched to the attempt that follows it.
#[derive(Deserialize)]
pub struct CallbackAuth {
  /// The listener password ThingSpace sends in clear inside every callback.
  #[serde(default)]
  pub password: Option<String>,
  /// ThingSpace's request id.
  #[serde(default, rename = "requestId")]
  pub request_id: Option<String>,
  /// ThingSpace's `callbackCount`: which delivery attempt this is, from 1.
  #[serde(default, rename = "callbackCount")]
  pub callback_count: Option<i64>,
}

/// Which configured listener password a callback presented.
#[derive(Debug, PartialEq)]
pub enum PasswordMatch {
  /// `THINGSPACE_CALLBACK_PASSWORD`.
  Current,
  /// `THINGSPACE_CALLBACK_PASSWORD_PREVIOUS`, set for a day after a rotation.
  Previous,
  /// Neither: the callback is refused.
  Neither,
}

/// Checks a callback's password against the current one and, while a rotation's grace window
/// holds it, the previous one. ThingSpace kept sending a replaced password for up to 13 min 55 s
/// after the new one was registered, so without the second an ordinary rotation loses uplinks.
/// Both comparisons run in constant time and always both run, so timing cannot say which matched.
pub fn match_callback_password(
  presented: &str,
  current: &str,
  previous: Option<&str>,
) -> PasswordMatch {
  let is_current = constant_time_eq(presented.as_bytes(), current.as_bytes());
  let is_previous =
    previous.is_some_and(|previous| constant_time_eq(presented.as_bytes(), previous.as_bytes()));
  match (is_current, is_previous) {
    (true, _) => PasswordMatch::Current,
    (false, true) => PasswordMatch::Previous,
    (false, false) => PasswordMatch::Neither,
  }
}

/// A ThingSpace `NiddService` callback, holding only what dovecote reads. `username` and
/// `password` are deliberately absent, so a parsed callback never carries the credential; the
/// request id and attempt are read once, by `CallbackAuth`, which also parses when this does not.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NiddCallback {
  /// The top-level identifiers, which name the device the way the request did.
  #[serde(default)]
  pub device_ids: Vec<CarrierId>,
  /// `Delivered`, `Queued`, `DeliveryFailed`, `ConfigCreated`, or a failure status.
  #[serde(default)]
  pub status: Option<String>,
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
  let mut line = String::with_capacity(context.len() + 18 + category.len() + column.len());
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
  /// A `HELLO`'s body, meant to be the claim key's 32 hex characters.
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

// ThingSpace refuses a downlink holding two adjacent NUL bytes, so every platform frame is
// NUL-free by construction: a nonzero type byte, a header of ASCII digits, a space and a newline,
// JSON as serde_json writes it (every control character escaped), and a hex tag.

/// A version as a header carries it. Only a misbehaving device reports a negative one, sent as 0.
fn header_version(version: i32) -> u32 {
  u32::try_from(version).unwrap_or(0)
}

/// Appends a header: two decimal integers, one space between them and a newline after.
fn push_header(frame: &mut Vec<u8>, first: u32, second: u32) {
  frame.extend_from_slice(first.to_string().as_bytes());
  frame.push(b' ');
  frame.extend_from_slice(second.to_string().as_bytes());
  frame.push(b'\n');
}

/// Digits in `value` written in decimal.
fn decimal_len(value: u32) -> usize {
  value.checked_ilog10().map_or(1, |log| log as usize + 1)
}

/// The largest `target_config` one `SHADOW` frame carries at these versions: the frame less its
/// type byte, header and tag. Never below `capsules::NIDD_MAX_TARGET_CONFIG_BYTES`, the cap at
/// ten-digit versions.
pub fn shadow_config_cap(target_version: i32, current_version: i32) -> usize {
  let header =
    decimal_len(header_version(target_version)) + decimal_len(header_version(current_version)) + 2;
  NIDD_MAX_FRAME_BYTES - 1 - header - NIDD_TAG_CHARS
}

/// An unsigned `SHADOW` frame: the type byte, the versions header, then the raw `target_config`.
pub fn shadow_frame(shadow: &PigeonShadow) -> Vec<u8> {
  let config = shadow.target_config.clone().into_inner();
  let mut frame = Vec::with_capacity(1 + NIDD_SHADOW_HEADER_MAX + config.len() + NIDD_TAG_CHARS);
  frame.push(FRAME_SHADOW);
  push_header(
    &mut frame,
    header_version(shadow.target_version),
    header_version(shadow.current_version),
  );
  frame.extend_from_slice(config.as_bytes());
  frame
}

/// An unsigned `STATUS` frame: the type byte and a header of the code and its argument.
pub fn status_frame(code: u8, arg: u32) -> Vec<u8> {
  let mut frame = Vec::with_capacity(1 + NIDD_STATUS_HEADER_MAX + NIDD_TAG_CHARS);
  frame.push(FRAME_STATUS);
  push_header(&mut frame, u32::from(code), arg);
  frame
}

/// Appends the frame's tag: the first 8 bytes of HMAC-SHA256 over the frame, keyed by the claim
/// key, as 16 lowercase hex characters. The device drops a platform frame whose tag does not
/// verify.
pub async fn sign_frame(
  key: &[u8; NIDD_CLAIM_KEY_BYTES],
  mut frame: Vec<u8>,
) -> Result<Vec<u8>, String> {
  let mac = super::stripe_webhook::hmac_sha256(key, &frame).await?;
  push_tag(&mut frame, &mac)?;
  Ok(frame)
}

/// Appends the first 8 bytes of `mac` as hex, the half of `sign_frame` the host tests reach.
fn push_tag(frame: &mut Vec<u8>, mac: &[u8]) -> Result<(), String> {
  let Some(tag) = mac.get(..NIDD_TAG_CHARS / 2) else {
    return Err("HMAC shorter than a frame tag".into());
  };
  for &byte in tag {
    frame.extend_from_slice(&hex_pair(byte));
  }
  Ok(())
}

/// The claim key a `HELLO` presents: its body as 32 hex characters. Anything else is `None`.
pub fn hello_key(body: &[u8]) -> Option<[u8; NIDD_CLAIM_KEY_BYTES]> {
  std::str::from_utf8(body).ok().and_then(claim_key_bytes)
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

/// A byte as two lowercase hex digits.
fn hex_pair(byte: u8) -> [u8; 2] {
  [
    HEX_DIGITS[usize::from(byte >> 4)],
    HEX_DIGITS[usize::from(byte & 0x0f)],
  ]
}

/// The de-duplication key of one uplink: the request id and the first 16 hex characters of the
/// frame's SHA-256. ThingSpace's retry of a callback, and a support resend of one, repeat both
/// halves.
pub fn dedupe_key(request_id: &str, frame: &[u8]) -> String {
  let digest = Sha256::digest(frame);
  let mut key = String::with_capacity(request_id.len() + 17);
  key.push_str(request_id);
  key.push(':');
  for &byte in &digest[..8] {
    key.extend(hex_pair(byte).map(char::from));
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
  /// The `target_version` of the last `SHADOW` sent; 0 after a send that never reached ThingSpace.
  pub pushed_version: i32,
  /// When that `SHADOW` was sent; 0 after a send that never reached ThingSpace.
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

/// Whether a dashboard write pushes the shadow unsolicited: the device is claimed and behind, the
/// newest target has not been sent, and no `SHADOW` went out in the last hold window. A push to a
/// device holding no connection can be lost, so the next uplink carries anything this holds back
/// or loses.
pub fn shadow_push_due(row: &NiddRow, now: i64) -> bool {
  row.claimed_at.is_some()
    && row.awaiting_version != 0
    && row.pushed_version < row.awaiting_version
    && now - row.pushed_at >= NIDD_PUSH_HOLD_SECS
}

/// Whether an uplink from the claimed device draws the `SHADOW` it is owed as its reply, sent
/// while that uplink's connection is up: the device is behind, and the newest target is unsent or
/// held, or went out longer ago than its delivery window without being confirmed.
pub fn shadow_reply_due(row: &NiddRow, now: i64) -> bool {
  row.claimed_at.is_some()
    && row.awaiting_version != 0
    && (row.pushed_version < row.awaiting_version || now - row.pushed_at > NIDD_MT_DELIVERY_SECS)
}

/// Whether this environment's `NIDD_ALLOWED_ORG_IDS` lists the organization, the create gate
/// that keeps NIDD to JES's own devices while every line rides JES's one ThingSpace account.
/// Empty or unset lists none, and a personal flock has no organization to list.
pub fn nidd_org_allowed(env: &worker::Env, org_id: &uuid::Uuid) -> bool {
  env
    .var("NIDD_ALLOWED_ORG_IDS")
    .is_ok_and(|raw| org_listed(&raw.to_string(), org_id))
}

/// Whether a comma-separated list of organization ids names this one, compared as UUIDs so case
/// and hyphenation cannot cause a false refusal. Unparseable entries name nothing.
fn org_listed(raw: &str, org_id: &uuid::Uuid) -> bool {
  raw
    .split(',')
    .filter_map(|entry| uuid::Uuid::parse_str(entry.trim()).ok())
    .any(|listed| listed == *org_id)
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

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::JsonString;

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
      device_ids: top,
      status: None,
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
    let auth: CallbackAuth = serde_json::from_str(body).unwrap();
    assert_eq!(auth.callback_count, None);
    assert_eq!(callback_imei(&callback), None);
  }

  #[test]
  fn an_unknown_variant_does_not_parse() {
    let body = r#"{"requestId":"r","niddResponse":{"niddSomethingElse":{"accountName":"a"}}}"#;
    assert!(serde_json::from_str::<NiddCallback>(body).is_err());
  }

  #[test]
  fn callback_auth_reads_the_request_id_and_attempt() {
    let auth: CallbackAuth = serde_json::from_str(&uplink_body()).unwrap();
    assert_eq!(auth.password.as_deref(), Some("pwd"));
    assert_eq!(
      auth.request_id.as_deref(),
      Some("a0fff7d6-6b30-45eb-84d7-0bc103d319c0")
    );
    assert_eq!(auth.callback_count, Some(1));

    // A body that is not a callback still names its attempt.
    let refused: CallbackAuth =
      serde_json::from_str(r#"{"requestId":"r","callbackCount":2,"niddResponse":7}"#).unwrap();
    assert_eq!(refused.callback_count, Some(2));

    let bare: CallbackAuth = serde_json::from_str("{}").unwrap();
    assert!(bare.password.is_none() && bare.request_id.is_none() && bare.callback_count.is_none());
  }

  #[test]
  fn the_previous_password_is_accepted_only_while_set() {
    assert_eq!(
      match_callback_password("new", "new", Some("old")),
      PasswordMatch::Current
    );
    assert_eq!(
      match_callback_password("old", "new", Some("old")),
      PasswordMatch::Previous
    );
    assert_eq!(
      match_callback_password("old", "new", None),
      PasswordMatch::Neither
    );
    assert_eq!(
      match_callback_password("other", "new", Some("old")),
      PasswordMatch::Neither
    );
    assert_eq!(
      match_callback_password("", "new", Some("old")),
      PasswordMatch::Neither
    );
    // Current wins when an operator left both secrets holding the same value.
    assert_eq!(
      match_callback_password("same", "same", Some("same")),
      PasswordMatch::Current
    );
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
  fn only_a_listed_organization_may_create() {
    let org = uuid::Uuid::parse_str("b30f82fd-a437-481b-9522-e52976022858").unwrap();
    assert!(org_listed("b30f82fd-a437-481b-9522-e52976022858", &org));
    assert!(org_listed(
      " 5bdd10e2-e079-48f0-8a81-49798d55e2f9 , B30F82FD-A437-481B-9522-E52976022858",
      &org
    ));
    assert!(!org_listed("", &org));
    assert!(!org_listed(
      "not-a-uuid, 5bdd10e2-e079-48f0-8a81-49798d55e2f9",
      &org
    ));
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

  /// The fixture key docs/api.md's exact bytes use: 16 zero bytes, never a real key.
  const FIXTURE_KEY: [u8; NIDD_CLAIM_KEY_BYTES] = [0; NIDD_CLAIM_KEY_BYTES];

  /// HMAC-SHA256 on the host target, so the tests reach a whole signed frame; dovecote itself
  /// signs through WebCrypto.
  fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let pad = |byte: u8| block.map(|k| k ^ byte);
    let inner = Sha256::new()
      .chain_update(pad(0x36))
      .chain_update(message)
      .finalize();
    Sha256::new()
      .chain_update(pad(0x5c))
      .chain_update(inner)
      .finalize()
      .to_vec()
  }

  fn signed(key: &[u8; NIDD_CLAIM_KEY_BYTES], mut frame: Vec<u8>) -> Vec<u8> {
    let mac = hmac_sha256(key, &frame);
    push_tag(&mut frame, &mac).unwrap();
    frame
  }

  fn shadow(target_version: i32, current_version: i32, config: &str) -> PigeonShadow {
    PigeonShadow {
      target_version,
      current_version,
      target_config: JsonString::new(config.to_string()).unwrap(),
      ..Default::default()
    }
  }

  #[test]
  fn the_host_hmac_matches_rfc_4231() {
    // RFC 4231 test case 2.
    let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    let hex: Vec<u8> = mac.iter().flat_map(|&b| hex_pair(b)).collect();
    assert_eq!(
      hex,
      b"5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
  }

  #[test]
  fn the_shadow_frame_matches_the_documented_bytes() {
    let frame = shadow_frame(&shadow(8, 7, r#"{"telemetry_interval":900,"log":true}"#));
    let mut expected = b"\x818 7\n".to_vec();
    expected.extend_from_slice(br#"{"telemetry_interval":900,"log":true}"#);
    assert_eq!(frame, expected);

    let whole = signed(&FIXTURE_KEY, frame);
    assert_eq!(whole.len(), 58);
    assert_eq!(&whole[42..], b"73ce8de7d438f541");
  }

  #[test]
  fn the_status_frames_match_the_documented_bytes() {
    for (code, arg, expected) in [
      (STATUS_STORED, 7, &b"\x820 7\ndb37ac5430ae1394"[..]),
      (
        STATUS_PAUSED,
        NIDD_PAUSED_HOLD_SECS,
        &b"\x821 3600\n1c273dd5d282365a"[..],
      ),
      (STATUS_UNCLAIMED, 0, &b"\x822 0\nedba557d1f9f3445"[..]),
      (STATUS_UNCLAIMED, 1, &b"\x822 1\n6e3162e1b9f30649"[..]),
    ] {
      assert_eq!(signed(&FIXTURE_KEY, status_frame(code, arg)), expected);
    }
  }

  #[test]
  fn a_hello_carries_the_claim_key_as_hex() {
    let mut frame = vec![FRAME_HELLO];
    frame.extend_from_slice(b"00000000000000000000000000000000");
    assert_eq!(frame.len(), 33);
    let Uplink::Hello(body) = decode_uplink(&frame) else {
      panic!("a HELLO decoded as another type");
    };
    assert_eq!(hello_key(body), Some(FIXTURE_KEY));
    assert_eq!(
      hello_key(b"00112233445566778899aabbccddeeff").map(|key| key[15]),
      Some(0xff)
    );

    // The retired raw form, a short key and junk claim nothing.
    assert_eq!(hello_key(&[0u8; 16]), None);
    assert_eq!(hello_key(b"00112233445566778899aabbccddeef"), None);
    assert_eq!(hello_key(b"00112233445566778899aabbccddeeg0"), None);
  }

  #[test]
  fn no_platform_frame_holds_two_adjacent_nul_bytes() {
    let keys = [
      FIXTURE_KEY,
      [0xff; NIDD_CLAIM_KEY_BYTES],
      *b"0123456789abcdef",
    ];
    let versions = [i32::MIN, -1, 0, 1, 7, 255, 256, 65_535, 65_536, i32::MAX];
    let configs = [r#"{}"#, r#"{"log":false}"#, r#"{"nul":"\u0000","n":0}"#];
    let args = [0, 1, 7, 256, 3600, 65_536, u32::MAX];
    let mut frames = Vec::new();
    for key in &keys {
      for &target in &versions {
        for &current in &versions {
          for config in configs {
            frames.push(signed(key, shadow_frame(&shadow(target, current, config))));
          }
        }
      }
      for code in [STATUS_STORED, STATUS_PAUSED, STATUS_UNCLAIMED, u8::MAX] {
        for arg in args {
          frames.push(signed(key, status_frame(code, arg)));
        }
      }
    }
    for frame in &frames {
      assert!(!frame.windows(2).any(|pair| pair == [0, 0]));
      assert!(!frame.contains(&0), "a NUL-free frame cannot hold two");
    }
  }

  #[test]
  fn the_shadow_header_fits_its_budget() {
    let widest = shadow_frame(&shadow(i32::MAX, i32::MAX, "{}"));
    assert_eq!(widest.len(), 1 + NIDD_SHADOW_HEADER_MAX + 2);
    let widest = status_frame(u8::MAX, u32::MAX);
    assert_eq!(widest.len(), 1 + NIDD_STATUS_HEADER_MAX);

    assert_eq!(
      shadow_config_cap(i32::MAX, i32::MAX),
      capsules::NIDD_MAX_TARGET_CONFIG_BYTES
    );
    assert_eq!(shadow_config_cap(8, 7), 1358 - 1 - 4 - 16);
    assert_eq!(shadow_config_cap(10, 10), 1358 - 1 - 6 - 16);
    assert_eq!(shadow_config_cap(1, -5), shadow_config_cap(1, 0));

    // A config at the cap makes a frame of exactly the frame limit.
    let config = format!(
      r#"{{"pad":"{}"}}"#,
      "x".repeat(shadow_config_cap(8, 8) - 10)
    );
    let whole = signed(&FIXTURE_KEY, shadow_frame(&shadow(8, 8, &config)));
    assert_eq!(whole.len(), NIDD_MAX_FRAME_BYTES);
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
  fn shadow_push_and_reply_truth_table() {
    let now = 1_000_000;
    let claimed = NiddRow {
      claimed_at: Some(1),
      awaiting_version: 8,
      ..Default::default()
    };

    // Never pushed: a dashboard write pushes it, and an uplink draws it.
    assert!(shadow_push_due(&claimed, now));
    assert!(shadow_reply_due(&claimed, now));

    // A dashboard write ten minutes after the last push is held, and the next uplink carries it.
    let held = NiddRow {
      pushed_version: 7,
      pushed_at: now - 600,
      ..claimed.clone()
    };
    assert!(!shadow_push_due(&held, now));
    assert!(shadow_push_due(&held, now - 600 + 900));
    assert!(shadow_reply_due(&held, now));

    // The newest version is out and inside its delivery window: nothing is sent twice.
    let in_flight = NiddRow {
      pushed_version: 8,
      pushed_at: now - NIDD_MT_DELIVERY_SECS,
      ..claimed.clone()
    };
    assert!(!shadow_push_due(&in_flight, now));
    assert!(!shadow_reply_due(&in_flight, now));

    // Past the window without the device confirming: the next uplink re-sends it, however soon,
    // and a dashboard write never does, however late.
    let lapsed = NiddRow {
      pushed_at: now - NIDD_MT_DELIVERY_SECS - 1,
      ..in_flight.clone()
    };
    assert!(shadow_reply_due(&lapsed, now));
    assert!(!shadow_push_due(&lapsed, now));
    assert!(!shadow_push_due(&lapsed, now + 86_400));

    // A send that never reached ThingSpace: due again for both.
    let unsent = NiddRow {
      pushed_version: 0,
      pushed_at: 0,
      ..claimed.clone()
    };
    assert!(shadow_push_due(&unsent, now));
    assert!(shadow_reply_due(&unsent, now));

    // Unclaimed, or converged: never, and a converged device's telemetry draws nothing.
    for row in [
      NiddRow {
        claimed_at: None,
        ..lapsed.clone()
      },
      NiddRow {
        awaiting_version: 0,
        ..lapsed
      },
    ] {
      assert!(!shadow_push_due(&row, now));
      assert!(!shadow_reply_due(&row, now));
    }
  }
}
